#!/bin/bash
# Multi-Market Logger Overnight Runner
# Auto-restarts the multi-market logger for 8 hours
#
# Usage:
#   ./run_overnight.sh [duration_hours] [output.csv] [num_markets]
#
# Example:
#   ./run_overnight.sh 8 overnight_btc_data.csv 10

set -e

# Parse arguments
DURATION_HOURS=${1:-8}
OUTPUT_CSV=${2:-overnight_btc_data_$(date +%Y%m%d_%H%M%S).csv}
NUM_MARKETS=${3:-10}

# Calculate duration in seconds
DURATION_SECS=$((DURATION_HOURS * 3600))

# Log file for this session
LOG_FILE="overnight_run_$(date +%Y%m%d_%H%M%S).log"

echo "================================"
echo "Multi-Market Logger Overnight Runner"
echo "================================"
echo "Duration: ${DURATION_HOURS} hours (${DURATION_SECS} seconds)"
echo "Output: ${OUTPUT_CSV}"
echo "Markets: ${NUM_MARKETS}"
echo "Log: ${LOG_FILE}"
echo "Start time: $(date)"
echo ""

# Create backup of existing CSV if it exists
if [ -f "${OUTPUT_CSV}" ]; then
    BACKUP_FILE="${OUTPUT_CSV}.backup_$(date +%Y%m%d_%H%M%S)"
    echo "Backing up existing CSV to: ${BACKUP_FILE}"
    cp "${OUTPUT_CSV}" "${BACKUP_FILE}"
fi

# Function to run the logger
run_logger() {
    local attempt=$1
    echo "[Attempt ${attempt}] Starting multi-market logger..."
    echo "[Attempt ${attempt}] Start time: $(date)"

    cargo run --bin multi_market_logger --release -- \
        "${DURATION_SECS}" \
        "${OUTPUT_CSV}" \
        "${NUM_MARKETS}" \
        2>&1 | tee -a "${LOG_FILE}"

    local exit_code=$?
    echo "[Attempt ${attempt}] Exit code: ${exit_code}"
    echo "[Attempt ${attempt}] End time: $(date)"

    return ${exit_code}
}

# Main loop with restart capability
ATTEMPT=1
MAX_ATTEMPTS=100  # Prevent infinite restart loops

while [ ${ATTEMPT} -le ${MAX_ATTEMPTS} ]; do
    echo ""
    echo "========================================"
    echo "Starting run attempt ${ATTEMPT} at $(date)"
    echo "========================================"

    if run_logger ${ATTEMPT}; then
        echo ""
        echo "========================================"
        echo "SUCCESS: Logger completed successfully!"
        echo "Total attempts: ${ATTEMPT}"
        echo "Output file: ${OUTPUT_CSV}"
        echo "Log file: ${LOG_FILE}"
        echo "End time: $(date)"
        echo "========================================"
        exit 0
    else
        EXIT_CODE=$?
        echo ""
        echo "========================================"
        echo "FAILED: Logger exited with code ${EXIT_CODE}"
        echo "Attempt ${ATTEMPT} failed at $(date)"
        echo "========================================"

        # Check if we should retry
        if [ ${ATTEMPT} -lt ${MAX_ATTEMPTS} ]; then
            NEXT_ATTEMPT=$((ATTEMPT + 1))
            WAIT_TIME=30  # Wait 30 seconds before retry
            echo "Waiting ${WAIT_TIME}s before attempt ${NEXT_ATTEMPT}..."
            sleep ${WAIT_TIME}
            ATTEMPT=${NEXT_ATTEMPT}
        else
            echo "Max attempts (${MAX_ATTEMPTS}) reached. Giving up."
            exit ${EXIT_CODE}
        fi
    fi
done

echo "Unexpected error in restart loop"
exit 1
