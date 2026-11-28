Changed
Per-thread build caches are now removed when a thread is archived (once idle) and automatically capped at the smaller of 20 GB or 10% of free disk space using oldest-used eviction, never removing an active thread's cache. Settings → Machines shows the current machine's build-cache size with a confirmed Clear control for idle threads.
