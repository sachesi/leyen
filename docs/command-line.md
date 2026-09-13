# The command line

`leyen` talks to the same daemon as the window, so a game started from a terminal shows as
running in the window and the other way round.

    leyen list             # the games, their Leyen IDs, and what is running
    leyen run ly-1a2b      # launch a game
    leyen kill ly-1a2b     # stop it
    leyen logs             # print the log
    leyen logs --follow    # and keep printing new lines until Ctrl+C

`list` reads the library itself and asks the daemon what is running:

    Games
      ly-1a2b  Elden Ring
      ly-3c4d  Hades

    Groups
    [group] Final Fantasy
      ly-5e6f  Final Fantasy VII Remake
      ly-7a8b  Final Fantasy X/X-2 HD Remaster

A running game is listed under Running with the process the daemon started and how many
processes belong to it. One deleted from the library while it ran is listed as
`<unknown>`.

`run` and `kill` say why when they fail: a Leyen ID that is not in the library, a game that
is not running, or the daemon's own reason, the one the window shows. A daemon that does
not answer within thirty seconds counts as a failure.

The menu entries Leyen writes run `leyen run <Leyen ID>`.

## Shell completions

`just install` installs completions for bash, fish and zsh. They complete the commands,
the Leyen IDs for `run`, the running ones for `kill`, and `--follow` for `logs`.

To use them from a checkout instead:

    source data/completions/leyen.bash                                   # bash
    install -Dm644 data/completions/leyen.fish ~/.config/fish/completions/leyen.fish
    fpath=(/path/to/leyen/data/completions $fpath); autoload -Uz compinit && compinit   # zsh
