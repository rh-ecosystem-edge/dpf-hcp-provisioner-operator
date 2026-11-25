#!/bin/bash

BFLOG="/usr/local/bin/bflog.sh"
MARKER="/run/dpu-agent/configuration-complete"
START_TIME=$(date +%s)

echo "INFO: Waiting for dpu-agent configuration marker at $MARKER..."
"$BFLOG" "INFO: Waiting for dpu-agent configuration marker at $MARKER..."

while true; do
  if [ -f "$MARKER" ]; then
    echo "INFO: dpu-agent configuration complete"
    "$BFLOG" "INFO: dpu-agent configuration complete"
    exit 0
  fi

  ELAPSED=$(( $(date +%s) - START_TIME ))
  echo "INFO: dpu-agent configuration not complete yet (elapsed=${ELAPSED}s)"
  sleep 2
done
