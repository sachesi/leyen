# Security

Please report a vulnerability privately rather than in a public issue: through
[a private advisory](https://github.com/sachesi/leyen/security/advisories/new) on GitHub,
or by mail to sachesi <xsachesi@pm.me>. Say what you found, how to reproduce it and which
version you ran; a fix is worked out with you before anything is published.

Only the latest release gets fixes.

## What counts

Leyen runs programs you point it at, and it runs them in a sandbox: bubblewrap with an
explicit list of what the program may see, and a seccomp filter over it. A game doing what
games do inside that sandbox is not a vulnerability; a way out of it is.

What the sandbox holds a game to: its Wine prefix and its game folder, read-write; `/usr`,
`/etc` and `/sys`, read-only; the device nodes for graphics, sound and controllers; the
display and audio sockets; a bus socket with nothing listening on it. `$HOME`, `/tmp` and
`$XDG_RUNTIME_DIR` are empty filesystems with only those paths bound back in, so Leyen's own
configuration — which decides what Leyen launches next — is not there at all. Games only
run sandboxed: where one cannot be built, the launch is refused rather than run unconfined.

What it deliberately still exposes, and what is therefore not a vulnerability: the whole of
`/dev/input`, so controllers work and can be plugged in while a game runs, which also means a
game can read input devices; the PipeWire and PulseAudio sockets, which carry the microphone
as well as the speakers, and on PipeWire any camera it offers; the X11 socket on an X11
session, where any client can watch the others; `/etc` and `/sys` as any program on the
system can read them; the network, unless it is switched off for that game, its group or all games; and the folders
you share under Extra Folders, which are yours to choose. A folder that would undo the
sandbox — your home directory, Leyen's own directories, a system directory, or any folder
holding one of them — is refused when the game launches, after the path is resolved, so a
symlink cannot stand in for one. A way past that check is a vulnerability.

The parts where a mistake matters:

- What Leyen downloads and runs by itself: umu-launcher and winetricks, each pinned to a
  release and checked against its SHA-256 before it is unpacked or installed, and fetched
  over HTTPS only. A way to make Leyen run something other than what it meant to fetch is
  a vulnerability.
- Files it reads that someone else wrote: the icons it extracts from game executables and
  the custom icons it is given are decoded with size limits; a file that makes Leyen crash,
  hang or write outside its icon directory is a vulnerability.
- Stopping a game: Leyen kills the processes in the game's systemd scope and the ones that
  match the game. Stopping one game and killing a process that is not that game's is a
  vulnerability.
- The sandbox itself: a launch that ends up with a path bound that the list above does not
  name, a game that can write where it should only read, a filter that is not applied, or any
  way to have Leyen start a program outside the sandbox.
- The menu entries and prefixes it writes: a title that turns into another command in a
  desktop entry, or a path that leads Leyen to write or delete outside the directories it
  was pointed at.

The daemon answers on your session bus, which only your own processes reach; a request
from your own session doing what that session could do anyway is not a vulnerability.
