# Security

Please report a vulnerability privately rather than in a public issue: through
[a private advisory](https://github.com/sachesi/leyen/security/advisories/new) on GitHub,
or by mail to sachesi <xsachesi@pm.me>. Say what you found, how to reproduce it and which
version you ran; a fix is worked out with you before anything is published.

Only the latest release gets fixes.

## What counts

Leyen runs programs you point it at, so a game doing what games do is not a vulnerability.
The parts where a mistake matters:

- What Leyen downloads and runs by itself: umu-launcher and winetricks, each pinned to a
  release and checked against its SHA-256 before it is unpacked or installed, and fetched
  over HTTPS only. A way to make Leyen run something other than what it meant to fetch is
  a vulnerability.
- Files it reads that someone else wrote: the icons it extracts from game executables and
  the custom icons it is given are decoded with size limits; a file that makes Leyen crash,
  hang or write outside its icon directory is a vulnerability.
- Stopping a game: Leyen kills the processes in the game's systemd scope and, in a shared
  container, the ones that match the game. Stopping one game and killing a process that
  is not that game's is a vulnerability.
- The menu entries and prefixes it writes: a title that turns into another command in a
  desktop entry, or a path that leads Leyen to write or delete outside the directories it
  was pointed at.

The daemon answers on your session bus, which only your own processes reach; a request
from your own session doing what that session could do anyway is not a vulnerability.
