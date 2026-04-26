# Shell Completions

Completion files in `packaging/usr/share/`.

## Bash

Temporary:
```bash
source packaging/usr/share/bash-completion/completions/leyen.bash
```

Permanent:
```bash
install -Dm644 packaging/usr/share/bash-completion/completions/leyen.bash ~/.local/share/bash-completion/completions/leyen
```

## Zsh

Add completion dir to `$fpath` in `.zshrc`:
```zsh
fpath=(/path/to/leyen/packaging/usr/share/zsh/site-functions $fpath)
autoload -Uz compinit && compinit
```

## Fish

```fish
install -Dm644 packaging/usr/share/fish/vendor_completions.d/leyen.fish ~/.config/fish/completions/leyen.fish
```