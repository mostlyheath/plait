Added
Projects on hosts with thread graph support (`PLAIT_GRAPH=1`) now offer a Graph tab showing linked threads and drafts, their states and dependencies, and draft Start and Delete controls.
Pending Start and Delete actions survive tab and machine switches. Restored tabs respect the host’s current Graph capability, and malformed host records no longer hide valid cards.
After reconnecting, a draft whose action was sent without a reply offers Retry Start or Retry Delete for that same action, even after the project is renamed. Large graphs draw only the visible portion of each dependency line.
