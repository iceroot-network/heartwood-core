#!/bin/sh
# docker.sh: the docker command of the harness. Set DIFFTEST_DOCKER to use another command.
#
# It runs docker directly when this user can reach the Docker daemon, and `sudo -n docker`
# otherwise; -n makes a missing sudo rule fail at once instead of prompting for a password.
if docker version >/dev/null 2>&1; then
    exec docker "$@"
fi
exec sudo -n docker "$@"
