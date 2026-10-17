#!/bin/bash
set +x
set -e
if ! command -v tmux >/dev/null; then
    printf 'This image has no tmux; select an image containing tmux, or stop this task with infra cli-stop.\n' >&2
    exit 1
fi
case ${TERM:-dumb} in
    dumb|'') export TERM=xterm-256color ;;
esac
# A fixed session name and socket let every ECS Exec client reach the same server.
# The server inherits the administrator URL built by admin-shell.sh.
exec tmux -L ultrafinance new-session -A -s ultrafinance '/bin/bash --rcfile /etc/bash.bashrc -i'
