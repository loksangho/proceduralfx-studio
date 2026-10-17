#!/bin/sh
# Sim caches are written relative to the working directory, so start in the
# app's own data directory (~/.var/app/<id>/data) instead of $HOME.
cd "${XDG_DATA_HOME:-$HOME}" || exit 1
exec /app/bin/proceduralfx_studio "$@"
