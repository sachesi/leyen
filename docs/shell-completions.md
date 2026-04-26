# Shell completions

Completion files are stored in:

```text
completions/
```

Files:

- `leyen.bash`
- `_leyen`
- `leyen.fish`

They complete public commands:

```text
help
list
run
logs
kill
```

`run` and `kill` complete Leyen IDs parsed from `leyen list`.

## Bash

Temporary for the current shell:

```bash
source completions/leyen.bash
```

User install:

```bash
install -Dm644 completions/leyen.bash ~/.local/share/bash-completion/completions/leyen
```

Then start a new shell.

## Zsh

User install:

```zsh
mkdir -p ~/.local/share/zsh/site-functions
install -Dm644 completions/_leyen ~/.local/share/zsh/site-functions/_leyen
```

Add this to `~/.zshrc` if not already configured:

```zsh
fpath=(~/.local/share/zsh/site-functions $fpath)
autoload -Uz compinit
compinit
```

Then start a new shell.

## Fish

User install:

```fish
install -Dm644 completions/leyen.fish ~/.config/fish/completions/leyen.fish
```

Fish loads completions automatically.
