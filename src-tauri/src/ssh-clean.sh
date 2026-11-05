#!/bin/sh
# Keep the local terminal usable when SSH exits, including after a sleep disconnect.
cleanup() {
    if [ -t 0 ]; then
        stty sane 2>/dev/null || :
    fi
    if [ -t 1 ]; then
        printf '\033[?1000l\033[?1002l\033[?1003l\033[?1006l\033[?1004l\033[?2004l\033[0m\033[?25h'
    fi
}
trap cleanup EXIT
# SSH shares our foreground process group and receives terminal signals directly.
# Keep the wrapper alive until SSH returns, without replacing its exit status.
trap ':' HUP INT TERM
ssh -o ServerAliveInterval=15 -o ServerAliveCountMax=3 "$@"
exit $?
