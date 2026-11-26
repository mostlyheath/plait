export const SETTINGS_CHANGED = 'Settings changed since this draft was made';
const draftFields = ['title', 'prompt', 'kind', 'start_when_ready', 'harness', 'provider', 'model_id', 'mode', 'thinking', 'approval', 'depends_on'];
export const fields: Record<string, string[]> = {
  start_draft: ['node_id'], graph: ['project', 'revision'], create_draft: ['checkout', 'project', ...draftFields], fork_thread: ['parent_id', 'at_message_id', 'brief', 'seed_mode', ...draftFields],
  delete_draft: ['node_id'], set_dependencies: ['node_id', 'depends_on'], set_node_kind: ['node_id', 'kind'],
  set_start_when_ready: ['node_id', 'start_when_ready'], send_results: ['parent_id']
};
export function graphRequest(op: unknown) { return typeof op === 'string' && Object.hasOwn(fields, op); }

export type Request = Record<string, unknown>;
export type Send = (request: Request) => Promise<unknown>;
