# Status Update: UI Still Freezes (Work in Progress)

## Observation
UI freezes after ~2 minutes of gameplay. 
Previous optimizations improved time-to-freeze from 30s to 2min.

## Status: Job NOT Done
Root cause is still being investigated.

## Recent Work (Failed to fully resolve)
1.  **Memory-Backed Logs**: Decoupled UI logs from disk I/O. UI now uses a memory-backed ring buffer (1000 lines).
2.  **Optimized Scanning**: Filtered /proc scans to skip system PIDs and non-game processes.
3.  **Reduced Polling**: Slowed down background monitor to once every 2 seconds.
4.  **GTK Optimization**: Disabled word-wrap in log window to reduce re-render cost.

## Current Investigation
- **Lock Contention**: Investigating if RwLock on the log buffer or running game cache is being held too long.
- **Save Pressure**: Checking if frequent config/library saves are blocking the UI indirectly.
- **Wait/Reap behavior**: Checking if zombie process reaping is causing kernel-level delays.
