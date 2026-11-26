Fixed
Threads started in a plain folder now detect when the folder becomes a Git
repository before the next run, protecting uncommitted edits and recording
turns for undo.
If Git appears during a plain-folder run, Pi refuses file edits and writes until the next message.
