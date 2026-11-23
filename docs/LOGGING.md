# Logging Strategy (#662)

This document specifies the structured, privacy-preserving logging strategy across the codebase.

## Overview

Structured logging standardizes error reporting, telemetry, and debugging across frontend clients, services, and backend systems while strictly ensuring private signer data and credentials remain unlogged.

## Log Event Format

Every log entry conforms to the structured JSON schema:

```json
{
  "timestamp": "2026-11-21T21:00:00.000Z",
  "level": "INFO",
  "message": "Transaction submitted successfully",
  "context": {
    "module": "transaction-relayer",
    "txId": "abcdef123456"
  }
}
```

## Log Levels

- **DEBUG**: Verbose internal diagnostics for development.
- **INFO**: Informational milestones (e.g. system startup, lifecycle state changes).
- **WARN**: Recoverable anomalies, non-critical faults, rate limiting notifications.
- **ERROR**: Unhandled exceptions or critical processing failures.

## Privacy Compliance

All logging sinks automatically sanitize metadata contexts. The following keys are masked with `[REDACTED]`:
- `secret`, `secretKey`, `privateKey`
- `password`, `token`, `auth`, `authorization`
- `seed`, `signature`

Signer secret keys and personal passwords must never appear in cleartext logs.
