#!/bin/bash

################################################################################
# Auto-Restart Script for Multi-Market Logger
################################################################################
# This script runs the multi-market logger binary and automatically restarts it
# if it crashes. Uses exponential backoff to prevent rapid restart loops.
#
# Usage: ./auto_restart_multi_market.sh [duration_secs] [output.csv] [num_markets]
#
# Features:
# - Exponential backoff on restart attempts
# - Comprehensive logging with timestamps
# - Clean shutdown on SIGINT/SIGTERM
# - Log rotation for restart logs
# - Infinite restart attempts (for overnight runs)
################################################################################

# -----------------------------------------------------------------------------
# Configuration Variables
# -----------------------------------------------------------------------------
BINARY_PATH="./target/release/multi_market_logger"
LOG_DIR="./logs"
MAX_BACKOFF_SECONDS=300
INITIAL_BACKOFF_SECONDS=2
BACKOFF_MULTIPLIER=2

# Create log directory if it doesn't exist
mkdir -p "${LOG_DIR}"

# Log file with rotation
TIMESTAMP=$(date +%Y%m%d_%H%M%S)
RESTART_LOG="${LOG_DIR}/auto_restart_${TIMESTAMP}.log"
LATEST_RESTART_LOG="${LOG_DIR}/auto_restart_latest.log"

# Create/clear latest log and add symlink
> "${LATEST_RESTART_LOG}"
ln -sf "$(basename "${RESTART_LOG}")" "${LOG_DIR}/auto_restart_latest.log" 2>/dev/null

# -----------------------------------------------------------------------------
# Helper Functions
################################################################################

# Logging function with timestamp
log() {
    local level="$1"
    shift
    local message="$*"
    local timestamp=$(date -Iseconds)
    echo "[${timestamp}] [AUTO_RESTART] [${level}] ${message}" | tee -a "${RESTART_LOG}" "${LATEST_RESTART_LOG}"
}

# Clean shutdown handler
cleanup() {
    log "INFO" "Shutdown signal received. Waiting for child process to exit cleanly..."

    if [ -n "${CHILD_PID}" ]; then
        # Send SIGINT to child process first
        kill -INT "${CHILD_PID}" 2>/dev/null

        # Wait up to 10 seconds for graceful shutdown
        local count=0
        while kill -0 "${CHILD_PID}" 2>/dev/null && [ ${count} -lt 10 ]; do
            sleep 1
            count=$((count + 1))
        done

        # Force kill if still running
        if kill -0 "${CHILD_PID}" 2>/dev/null; then
            log "WARN" "Process did not exit gracefully, forcing termination..."
            kill -9 "${CHILD_PID}" 2>/dev/null
        fi
    fi

    log "INFO" "Auto-restart script stopped cleanly"
    exit 0
}

# Trap signals for clean shutdown
trap cleanup SIGINT SIGTERM

# -----------------------------------------------------------------------------
# Validation
# -----------------------------------------------------------------------------

# Check if binary exists
if [ ! -f "${BINARY_PATH}" ]; then
    log "ERROR" "Binary not found at ${BINARY_PATH}"
    log "ERROR" "Please build the project first: cargo build --release"
    exit 1
fi

# Check if binary is executable
if [ ! -x "${BINARY_PATH}" ]; then
    log "ERROR" "Binary is not executable: ${BINARY_PATH}"
    log "ERROR" "Run: chmod +x ${BINARY_PATH}"
    exit 1
fi

# -----------------------------------------------------------------------------
# Main Loop
# -----------------------------------------------------------------------------

log "INFO" "=========================================="
log "INFO" "Auto-restart script started"
log "INFO" "Binary: ${BINARY_PATH}"
log "INFO" "Max backoff: ${MAX_BACKOFF_SECONDS}s"
log "INFO" "Initial backoff: ${INITIAL_BACKOFF_SECONDS}s"
log "INFO" "Arguments: $*"
log "INFO" "=========================================="

RESTART_COUNT=0
BACKOFF_SECONDS=${INITIAL_BACKOFF_SECONDS}

while true; do
    log "INFO" "Starting binary (attempt #${RESTART_COUNT})"
    log "INFO" "Command: ${BINARY_PATH} $*"

    # Record start time
    START_TIME=$(date +%s)

    # Run the binary and capture exit code
    "${BINARY_PATH}" "$@" &
    CHILD_PID=$!

    # Wait for child process
    wait ${CHILD_PID}
    EXIT_CODE=$?

    # Record end time
    END_TIME=$(date +%s)
    DURATION=$((END_TIME - START_TIME))

    # Check if exit was intentional (SIGINT/SIGTERM)
    if [ ${EXIT_CODE} -eq 130 ] || [ ${EXIT_CODE} -eq 143 ]; then
        log "INFO" "Process terminated by user (exit code: ${EXIT_CODE})"
        break
    fi

    # Process crashed or exited unexpectedly
    RESTART_COUNT=$((RESTART_COUNT + 1))

    log "ERROR" "Process exited unexpectedly!"
    log "ERROR" "Exit code: ${EXIT_CODE}"
    log "ERROR" "Uptime: ${DURATION} seconds"
    log "ERROR" "Restart attempt #${RESTART_COUNT} in ${BACKOFF_SECONDS} seconds..."

    # Wait with backoff
    sleep "${BACKOFF_SECONDS}"

    # Calculate next backoff (exponential)
    BACKOFF_SECONDS=$((BACKOFF_SECONDS * BACKOFF_MULTIPLIER))

    # Cap at maximum backoff
    if [ ${BACKOFF_SECONDS} -gt ${MAX_BACKOFF_SECONDS} ]; then
        BACKOFF_SECONDS=${MAX_BACKOFF_SECONDS}
        log "INFO" "Reached maximum backoff of ${MAX_BACKOFF_SECONDS} seconds"
    fi

    log "INFO" "------------------------------------------"
done

log "INFO" "Auto-restart loop terminated"
log "INFO" "Total restarts: ${RESTART_COUNT}"

exit 0
