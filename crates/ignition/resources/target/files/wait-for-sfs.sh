#!/bin/bash

exec > >(tee >(while read -r line; do /usr/local/bin/bflog.sh "$line"; done)) 2>&1

FLAVOR_FILE="/etc/dpf/dpuflavor.yaml"

EXPECTED=$(grep -oP 'PF_TOTAL_SF=\K[0-9]+' "$FLAVOR_FILE" 2>/dev/null)
if [ -z "$EXPECTED" ] || [ "$EXPECTED" -eq 0 ]; then
  echo "INFO: PF_TOTAL_SF not set or is 0, no SFs expected — passing gate"
  exit 0
fi

START_TIME=$(date +%s)
echo "INFO: Waiting for $EXPECTED SFs to be created..."

while true; do
  ACTUAL=$(mlnx-sf -a show -j 2>/dev/null | jq 'length // 0')
  if [ "${ACTUAL:-0}" -ge "$EXPECTED" ]; then
    echo "INFO: All SFs ready (expected=$EXPECTED, actual=$ACTUAL)"
    exit 0
  fi

  ELAPSED=$(( $(date +%s) - START_TIME ))
  echo "INFO: SFs not ready yet (expected=$EXPECTED, actual=${ACTUAL:-0}, elapsed=${ELAPSED}s)"
  sleep 5
done
