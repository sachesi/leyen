//! The seccomp filter every sandboxed process runs under.
//!
//! bubblewrap takes a filter as a compiled classic-BPF program on a file
//! descriptor, so the program is assembled here and handed to `bwrap` on
//! [`SECCOMP_FD`] through a `memfd` — nothing touches the filesystem.
//!
//! The filter is a deny list, not an allow list. Proton drags in wine, the Steam
//! Linux Runtime and pressure-vessel's own `bwrap`, which between them use most
//! of the system call table: `mount`, `pivot_root`, `unshare` with
//! `CLONE_NEWUSER` and `ptrace` (wineserver reads and writes thread contexts with
//! it) all have to stay reachable, so an allow list would either be a list of
//! everything or a launcher that runs no games. What the list removes is kernel
//! attack surface no game needs: the kernel keyring, eBPF, perf events, module
//! loading, `kexec`, `io_uring`, `userfaultfd`, swap, accounting and the handle
//! and notification interfaces.
//!
//! Filtering is by system call *number*, and those differ per ABI, so the
//! program dispatches on `seccomp_data.arch` and carries one table per ABI:
//! x86-64, i386 (32-bit wine processes make i386 calls even on a 64-bit host) and
//! the asm-generic numbers used by aarch64. An architecture that is none of the
//! three is allowed through — it cannot be filtered by numbers we do not have —
//! and the x32 ABI is refused outright, since its numbers are the x86-64 ones
//! with a bit set and would otherwise slip past the table.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;

/// File descriptor `bwrap` is told to read the filter from (`--seccomp`).
pub const SECCOMP_FD: i32 = 3;

// Classic-BPF instruction classes, as seccomp(2) uses them.
const LD_W_ABS: u16 = 0x20; // A = seccomp_data[k]
const JEQ_K: u16 = 0x15; // if A == k jump
const JGE_K: u16 = 0x35; // if A >= k jump
const JA: u16 = 0x05; // unconditional jump
const RET_K: u16 = 0x06; // return k

// Offsets into `struct seccomp_data`.
const DATA_NR: u32 = 0;
const DATA_ARCH: u32 = 4;

// Values of `seccomp_data.arch` (AUDIT_ARCH_*).
const ARCH_X86_64: u32 = 0xc000_003e;
const ARCH_I386: u32 = 0x4000_0003;
const ARCH_AARCH64: u32 = 0xc000_00b7;

/// x32 system calls are the x86-64 numbers with bit 30 set.
const X32_BIT: u32 = 0x4000_0000;

const RET_ALLOW: u32 = 0x7fff_0000;
const RET_ERRNO: u32 = 0x0005_0000;
const EPERM: u32 = 1;
const ENOSYS: u32 = 38;

/// One refused system call and its number in each ABI the filter knows.
/// `None` where an ABI never had the call.
struct Refused {
    x86_64: Option<u32>,
    i386: Option<u32>,
    generic: Option<u32>,
}

const fn nr(x86_64: Option<u32>, i386: Option<u32>, generic: Option<u32>) -> Refused {
    Refused {
        x86_64,
        i386,
        generic,
    }
}

/// Refused with `EPERM`: privileged or kernel-surface calls. A game that asks
/// for one is doing something it has no business doing, and "not permitted" is
/// what it would get from an unprivileged process on a locked-down kernel too.
const REFUSED_EPERM: &[Refused] = &[
    nr(Some(321), Some(357), Some(280)), // bpf
    nr(Some(298), Some(336), Some(241)), // perf_event_open
    nr(Some(250), Some(288), Some(219)), // keyctl
    nr(Some(248), Some(286), Some(217)), // add_key
    nr(Some(249), Some(287), Some(218)), // request_key
    nr(Some(246), Some(283), Some(104)), // kexec_load
    nr(Some(320), None, Some(294)),      // kexec_file_load
    nr(Some(175), Some(128), Some(105)), // init_module
    nr(Some(313), Some(350), Some(273)), // finit_module
    nr(Some(176), Some(129), Some(106)), // delete_module
    nr(Some(167), Some(87), Some(224)),  // swapon
    nr(Some(168), Some(115), Some(225)), // swapoff
    nr(Some(163), Some(51), Some(89)),   // acct
    nr(Some(179), Some(131), Some(60)),  // quotactl
    nr(Some(103), Some(103), Some(116)), // syslog (the kernel ring buffer)
    nr(Some(134), Some(86), None),       // uselib
    nr(Some(136), Some(62), None),       // ustat
    nr(Some(139), Some(135), None),      // sysfs
    nr(Some(153), Some(111), Some(58)),  // vhangup
    nr(Some(212), Some(253), Some(18)),  // lookup_dcookie
    nr(Some(304), Some(342), Some(265)), // open_by_handle_at
    nr(Some(300), Some(338), Some(262)), // fanotify_init
    nr(Some(172), Some(110), None),      // iopl
    nr(Some(173), Some(101), None),      // ioperm
];

/// Refused with `ENOSYS`: interfaces a library may probe for and do without.
/// "Not implemented" is what it sees on a kernel built without them, so the
/// probe fails the way the code already expects.
const REFUSED_ENOSYS: &[Refused] = &[
    nr(Some(425), Some(425), Some(425)), // io_uring_setup
    nr(Some(426), Some(426), Some(426)), // io_uring_enter
    nr(Some(427), Some(427), Some(427)), // io_uring_register
    nr(Some(323), Some(374), Some(282)), // userfaultfd
];

/// A jump whose target is known only once every instruction is emitted.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Allow,
    Eperm,
    Enosys,
    Arch(usize),
    None,
}

struct Instruction {
    code: u16,
    k: u32,
    /// Where a taken jump goes (`JEQ`/`JGE`: the true branch; `JA`: the only
    /// branch). The false branch is always the next instruction.
    target: Target,
}

/// Assembles the filter and returns it as the byte image of an array of
/// `struct sock_filter`, which is what `bwrap --seccomp` expects to read.
pub fn filter_bytes() -> Vec<u8> {
    let arches = [ARCH_X86_64, ARCH_I386, ARCH_AARCH64];
    let mut program: Vec<Instruction> = Vec::new();

    // Dispatch on the ABI. Anything unknown is allowed: its numbers mean
    // something else entirely, and guessing would refuse arbitrary calls.
    program.push(insn(LD_W_ABS, DATA_ARCH, Target::None));
    for (index, arch) in arches.iter().enumerate() {
        program.push(insn(JEQ_K, *arch, Target::Arch(index)));
    }
    program.push(insn(JA, 0, Target::Allow));

    let mut blocks = Vec::new();
    for (index, arch) in arches.iter().enumerate() {
        blocks.push((index, program.len()));
        program.push(insn(LD_W_ABS, DATA_NR, Target::None));
        if *arch == ARCH_X86_64 {
            // x32 calls carry bit 30; the table below would read them as
            // unrelated x86-64 numbers.
            program.push(insn(JGE_K, X32_BIT, Target::Eperm));
        }
        for refused in REFUSED_EPERM {
            if let Some(number) = number_for(refused, *arch) {
                program.push(insn(JEQ_K, number, Target::Eperm));
            }
        }
        for refused in REFUSED_ENOSYS {
            if let Some(number) = number_for(refused, *arch) {
                program.push(insn(JEQ_K, number, Target::Enosys));
            }
        }
        program.push(insn(JA, 0, Target::Allow));
    }

    let allow = program.len();
    program.push(insn(RET_K, RET_ALLOW, Target::None));
    let eperm = program.len();
    program.push(insn(RET_K, RET_ERRNO | EPERM, Target::None));
    let enosys = program.len();
    program.push(insn(RET_K, RET_ERRNO | ENOSYS, Target::None));

    let resolve = |target: Target| -> usize {
        match target {
            Target::Allow => allow,
            Target::Eperm => eperm,
            Target::Enosys => enosys,
            Target::Arch(index) => blocks[index].1,
            Target::None => 0,
        }
    };

    let mut bytes = Vec::with_capacity(program.len() * 8);
    for (index, instruction) in program.iter().enumerate() {
        let (jt, k) = match instruction.target {
            Target::None => (0u8, instruction.k),
            target => {
                let distance = resolve(target) - (index + 1);
                match instruction.code {
                    // A `JA` offset is the whole 32-bit k field, so it never
                    // runs out of reach; a conditional jump has one byte.
                    JA => (0u8, distance as u32),
                    _ => {
                        assert!(
                            distance <= u8::MAX as usize,
                            "seccomp jump out of reach: {distance}"
                        );
                        (distance as u8, instruction.k)
                    }
                }
            }
        };
        bytes.extend_from_slice(&instruction.code.to_ne_bytes());
        bytes.push(jt);
        bytes.push(0); // false branch: fall through
        bytes.extend_from_slice(&k.to_ne_bytes());
    }
    bytes
}

fn insn(code: u16, k: u32, target: Target) -> Instruction {
    Instruction { code, k, target }
}

fn number_for(refused: &Refused, arch: u32) -> Option<u32> {
    match arch {
        ARCH_X86_64 => refused.x86_64,
        ARCH_I386 => refused.i386,
        _ => refused.generic,
    }
}

/// The compiled filter, held on an anonymous file so it can be handed to
/// `bwrap` as a file descriptor without a temporary file anyone could swap.
pub struct SeccompFilter {
    file: File,
}

impl SeccompFilter {
    pub fn compile() -> io::Result<Self> {
        use std::io::{Seek, SeekFrom, Write};

        let name = c"leyen-seccomp";
        // SAFETY: `name` is a valid NUL-terminated string and the call only
        // returns a new descriptor or -1.
        let fd = unsafe { libc::memfd_create(name.as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor this call owns.
        let mut file = unsafe { File::from_raw_fd(fd) };
        file.write_all(&filter_bytes())?;
        file.seek(SeekFrom::Start(0))?;
        Ok(Self { file })
    }

    /// Arranges for the filter to be on [`SECCOMP_FD`] in the spawned child.
    /// The descriptor is placed after `fork` and before `exec`, so it survives
    /// into `bwrap` (and through the `systemd-run` in front of it) while the
    /// daemon's own descriptors stay untouched.
    pub fn place_on_fd(&self, command: &mut tokio::process::Command) -> io::Result<()> {
        self.place_on_std_fd(command.as_std_mut())
    }

    /// [`place_on_fd`](Self::place_on_fd) for a plain [`std::process::Command`].
    pub fn place_on_std_fd(&self, command: &mut std::process::Command) -> io::Result<()> {
        let file = self.file.try_clone()?;
        // SAFETY: the closure runs between fork and exec and calls only
        // async-signal-safe functions (dup2, fcntl, lseek).
        unsafe {
            command.pre_exec(move || {
                let fd = file.as_raw_fd();
                // dup2 onto itself would keep close-on-exec set.
                let placed = if fd == SECCOMP_FD {
                    libc::fcntl(fd, libc::F_SETFD, 0)
                } else {
                    libc::dup2(fd, SECCOMP_FD)
                };
                if placed < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::lseek(SECCOMP_FD, 0, libc::SEEK_SET) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every instruction is 8 bytes, and the program ends in the three returns.
    #[test]
    fn the_filter_is_a_well_formed_instruction_array() {
        let bytes = filter_bytes();
        assert_eq!(bytes.len() % 8, 0);
        let instructions = bytes.len() / 8;
        assert!(instructions > 3 * REFUSED_EPERM.len());
        let last_three = &bytes[bytes.len() - 24..];
        for chunk in last_three.chunks(8) {
            assert_eq!(u16::from_ne_bytes([chunk[0], chunk[1]]), RET_K);
        }
    }

    /// Applying the real filter must refuse the listed calls and nothing else.
    /// Runs in a forked child, which the filter then outlives only as long as
    /// the check takes.
    #[test]
    fn the_filter_refuses_the_listed_calls_and_allows_the_rest() {
        let bytes = filter_bytes();
        let program = libc::sock_fprog {
            len: (bytes.len() / 8) as u16,
            filter: bytes.as_ptr() as *mut libc::sock_filter,
        };

        // SAFETY: the child calls only async-signal-safe functions and leaves
        // with `_exit`, never returning into the test harness.
        let pid = unsafe { libc::fork() };
        assert!(pid >= 0, "fork failed");
        if pid == 0 {
            unsafe {
                if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                    libc::_exit(10);
                }
                if libc::syscall(
                    libc::SYS_seccomp,
                    1, // SECCOMP_SET_MODE_FILTER
                    0,
                    &program as *const libc::sock_fprog,
                ) != 0
                {
                    libc::_exit(11);
                }
                // bpf(2): refused with EPERM.
                if libc::syscall(libc::SYS_bpf, 0, 0, 0) != -1 || errno() != libc::EPERM {
                    libc::_exit(12);
                }
                // io_uring_setup(2): refused as not implemented.
                if libc::syscall(libc::SYS_io_uring_setup, 0, 0) != -1 || errno() != libc::ENOSYS {
                    libc::_exit(13);
                }
                // ptrace(2) stays reachable: wineserver needs it. PTRACE_PEEKDATA
                // on no process fails with ESRCH; what it must never be is the
                // filter's EPERM.
                if libc::syscall(libc::SYS_ptrace, 2, 0, 0, 0) == -1 && errno() == libc::EPERM {
                    libc::_exit(14);
                }
                // An ordinary call is untouched.
                if libc::syscall(libc::SYS_getpid) <= 0 {
                    libc::_exit(15);
                }
                libc::_exit(0);
            }
        }

        let mut status = 0;
        // SAFETY: waiting on the child just forked.
        unsafe { libc::waitpid(pid, &mut status, 0) };
        let exited = libc::WIFEXITED(status);
        let code = libc::WEXITSTATUS(status);
        assert!(
            exited && code == 0,
            "child reported {code} (exited {exited})"
        );
    }

    fn errno() -> i32 {
        io::Error::last_os_error().raw_os_error().unwrap_or(0)
    }
}
