#!/usr/bin/env bash
# TeammateIdle: nudge idle teammates to pick up unclaimed work rather than stop.
# Exit 2 keeps the teammate working with the stderr message as instruction.
echo "Before going idle: check the shared task list for unclaimed tasks matching your role, and message the lead with a one-line status." >&2
exit 2
