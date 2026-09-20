# Using Leyen

What follows is the parts worth knowing and the places where Leyen does something of its own.

## The library

The library lists groups first, then games, each by title; a game that is running moves to
the top, outlined in the accent colour, with how long it has been running. Every game shows
its playtime and when it was last played, every group how many games it holds, how many of
them run and when one was last played.

Clicking a game, or pressing Enter on it, launches it; doing it again stops it. The round
button does the same. Clicking a group opens it as a page of its own, with the group's title in the
header, a button to edit it and one to add a game to it. Back, Escape or a swipe returns to
the library.

Search (Ctrl+F, or just typing) looks through the titles of every game, including the ones
inside groups, and through the titles of groups: a group that matches lists all its games.

Closing the window while a game runs only hides it; opening Leyen again brings it back. It
closes by itself once the last game ends.

## Adding and editing games

"+" in the header adds a game or a group; inside a group it adds a game to that group. A game
needs a title, an executable and a game folder. The game folder is the only one of your files
the game will see, so Leyen does not guess it: name the game's install root, which for some
games is the folder the executable sits in and for others — `Binaries/Win64/game.exe` with
the content beside it — the folder above. Everything else has a default:

- **Leyen ID** and **Game ID** are given to the game once and never change. The Leyen ID
  (`ly-1234`) is what the command line and the menu entries use; the Game ID (`umu-ly1234`)
  is what umu-launcher sees as `GAMEID`.
- **Custom Icon** takes a PNG, JPEG or ICO file. Without one the icon is read from the
  executable; if the executable has none, a plain symbol stands in.
- **Custom Prefix** gives the game a Wine prefix of its own. Switched on, it suggests a
  folder named after the title beside the default prefix, and follows the title until the
  folder is edited. Switched off, the game uses its group's prefix or the default one.
- **Proton** is a Proton build from `~/.local/share/leyen/proton`, or Default, which leaves
  the choice to the group and the preferences, and in the end to umu-launcher's own build.
  A game in a group has **Custom Proton** instead, off while it inherits the group's.
- **Launch Arguments** are passed to the game. `%command%` stands for the game itself, so
  anything before it wraps the launch, as on Steam:
  `DXVK_HUD=fps %command% -windowed`.
- **Environment**: MangoHud, Wayland, WoW64, NTSync, HDR and a Proton log, each
  on or off for this game. A new game starts with the switches from the preferences.
- **Sandbox** is what the game may reach besides its own folder. **Network Access** is
  Default, which takes the answer from the group and then the preferences, Allowed, or
  Blocked. **Extra Folders** are folders to share on top: mods kept elsewhere, assets on
  another drive, a save folder of your own, each read-only unless switched writable. The
  group and the preferences share folders with every game the same way.

Adding a game also adds it to the applications menu. **Menu Entry** in its settings removes
it or adds it again; the entry runs `leyen run <Leyen ID>` and is named after the game, or
after its group and the game. Renaming a game or its group updates the entry.

Deleting a game deletes its playtime with it. Deleting a group deletes its games too, and
the alert says how many. A game that is running cannot be deleted, nor a group with a game
running: stop it first.

## Groups

A group gives its games a prefix, a Proton and a network answer to inherit, and has an icon of
its own. A game that has its own prefix, Proton or network answer keeps it. Editing a group's
title renames the menu entries of its games.

## Prefix tools

The settings of a game with its own prefix, of a group with a prefix, and the preferences
for the default prefix have the same tools:

- **Wine Configuration** and **Registry Editor** open `winecfg` and `regedit` in the prefix.
- **Manage Dependencies** installs winetricks components into the prefix: runtimes,
  fonts, codecs and the rest, installed ones first. Only one runs at a time, an install can
  be cancelled, and a component that another installed one needs says so when asked to go.
  Leaving the page does not stop what is running.
- **Run a Program…** runs a Windows program in the prefix, an installer say.

None of them run while a game does: a prefix changing under a running game is how prefixes
break. The other way round too: while Wine Configuration, the Registry Editor or a program
run, and until everything they started has ended, no game starts on that prefix and no
dependency is installed or removed. What they print is in the log. A game that uses its group's or the default prefix points there instead of offering
the tools, and a group whose games keep prefixes of their own names them.

## The sandbox

Every game runs confined, and so does everything else that runs Windows code: `winecfg`, the
Registry Editor, a program run by hand and a dependency install. There is no switch for it —
where a sandbox cannot be built, the launch is refused instead.

What a game can reach:

- its Wine prefix, read-write. Saves, the registry and the caches are there: what the game
  sees as `~/.cache` is `<prefix>/.leyen/cache`, so shader caches survive a run without
  reaching the rest of your home directory.
- its game folder, the one named in its settings, read-write.
- the folders shared with it under Extra Folders, in its settings, its group's or the
  preferences, read-only unless switched writable. A folder named at more than one level
  keeps what the narrowest one says, so a game can write in a folder the preferences share
  read-only.
- `/usr`, `/etc` and `/sys`, read-only, and the devices for graphics, sound and controllers.
- the display and audio sockets, and the network unless it is switched off.
- a bus socket with nothing listening on it, so a game asking for the session bus is
  refused as it would be on a machine without one. Your session bus is not in the sandbox: a
  game on it could tell Leyen to launch anything, or your systemd to start something outside
  the sandbox.

Games on the same prefix run side by side, several windows of one game included. Each has
a sandbox of its own, with its own folders and network setting; they share a process
namespace and a wineserver, because Wine serves a prefix from one.

What is not there: your home directory, `/tmp`, Leyen's own configuration, umu-launcher and
winetricks as anything but read-only copies, and the games you did not start. Some folders
cannot be shared at all, whatever the settings say — your home directory itself, Leyen's own
directories, `/etc`, `/usr` and the other system directories, and any folder holding one of
them. Naming one is refused when the game launches, with the reason in the log, and the game
starts without it.

Two games on one prefix each get a pressure-vessel container of their own — a game cannot
join another's container without also joining its sandbox, which holds the other game's
folder and not its own.

## Running games and logs

Running Games in the main menu lists what the daemon is tracking: the process it started,
how many processes belong to the game, how long it has run, its log and a stop button.

Logs shows what games print and what Leyen itself does, for every game or one, following new
lines as they arrive. The daemon keeps the last thousand lines; the log is also written to
`~/.config/leyen/logs.jsonl`. What games print is always kept; which of Leyen's own errors,
warnings and operations are is set under Logging in the preferences.

## Menu entries and the command line

A game can be started without the window: from its menu entry, or with `leyen run` in a
terminal. See [the command line](command-line.md).
