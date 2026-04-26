# Bash completion for leyen

_leyen_ids() {
    leyen list 2>/dev/null | awk '/^[[:space:]]+ly-[0-9][0-9][0-9][0-9][[:space:]]/ { print $1 }'
}

_leyen() {
    local cur prev words cword
    _init_completion -n : 2>/dev/null || {
        COMPREPLY=()
        cur="${COMP_WORDS[COMP_CWORD]}"
        prev="${COMP_WORDS[COMP_CWORD-1]}"
    }

    local commands="help list run logs kill --help -h"

    if [[ ${COMP_CWORD} -eq 1 ]]; then
        COMPREPLY=( $(compgen -W "${commands}" -- "${cur}") )
        return 0
    fi

    case "${prev}" in
        run|kill)
            COMPREPLY=( $(compgen -W "$(_leyen_ids)" -- "${cur}") )
            return 0
            ;;
    esac

    case "${COMP_WORDS[1]}" in
        run|kill)
            COMPREPLY=( $(compgen -W "$(_leyen_ids)" -- "${cur}") )
            return 0
            ;;
        help|list|logs|--help|-h)
            COMPREPLY=()
            return 0
            ;;
    esac
}

complete -F _leyen leyen
