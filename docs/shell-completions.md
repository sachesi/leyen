# Shell Completions

Completion files are in `data/completions/`; `just install` puts them where the shells look.

## Bash

Temporary:
```bash
source data/completions/leyen.bash
```

Permanent:
```bash
install -Dm644 data/completions/leyen.bash ~/.local/share/bash-completion/completions/leyen
```

## Zsh

Add completion dir to `$fpath` in `.zshrc`:
```zsh
fpath=(/path/to/leyen/data/completions $fpath)
autoload -Uz compinit && compinit
```

## Fish

```fish
install -Dm644 data/completions/leyen.fish ~/.config/fish/completions/leyen.fish
```