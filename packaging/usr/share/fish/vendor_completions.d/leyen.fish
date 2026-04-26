# Fish completion for leyen

function __fish_leyen_ids
    leyen list 2>/dev/null | awk '/^[[:space:]]+ly-[0-9][0-9][0-9][0-9][[:space:]]/ { print $1 }'
end

complete -c leyen -f

complete -c leyen -n '__fish_use_subcommand' -a help -d 'Show usage'
complete -c leyen -n '__fish_use_subcommand' -a list -d 'List games and running sessions'
complete -c leyen -n '__fish_use_subcommand' -a run -d 'Launch a game by Leyen ID'
complete -c leyen -n '__fish_use_subcommand' -a logs -d 'Open the log window'
complete -c leyen -n '__fish_use_subcommand' -a kill -d 'Stop a running game by Leyen ID'

complete -c leyen -n '__fish_seen_subcommand_from run' -a '(__fish_leyen_ids)' -d 'Leyen ID'
complete -c leyen -n '__fish_seen_subcommand_from kill' -a '(__fish_leyen_ids)' -d 'Leyen ID'

complete -c leyen -s h -l help -d 'Show usage'
