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
needs a title and an executable. Everything else has a default:

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
- **Environment**: MangoHud, GameMode, Wayland, WoW64, NTSync, HDR and a Proton log, each
  on or off for this game. A new game starts with the switches from the preferences.

Adding a game also adds it to the applications menu. **Menu Entry** in its settings removes
it or adds it again; the entry runs `leyen run <Leyen ID>` and is named after the game, or
after its group and the game. Renaming a game or its group updates the entry.

Deleting a game deletes its playtime with it. Deleting a group deletes its games too, and
the alert says how many.

## Groups

A group gives its games a prefix and a Proton to inherit, and has an icon of its own. A game
that has its own prefix or Proton keeps it. Editing a group's title renames the menu entries
of its games.

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
break. A game that uses its group's or the default prefix points there instead of offering
the tools, and a group whose games keep prefixes of their own names them.

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
