import type { Snapshot } from '../protocol.ts';
import type { CodexHarness } from '../harness.ts';
import type { ClaudeHarness } from '../claude/harness.ts';

export class ExternalHarnesses {
  private readonly runtime: { codex: CodexHarness; claude: ClaudeHarness };
  constructor(runtime: { codex: CodexHarness; claude: ClaudeHarness }) { this.runtime = runtime; }
  resolveHarness(snapshot: Snapshot) {
    const id: string = snapshot.harness?.id ?? 'pi';
    switch (id) {
      case 'pi': return { kind: 'pi' } as const;
      case 'codex': return { kind: 'external', id, harness: this.runtime.codex } as const;
      case 'claude': return { kind: 'external', id, harness: this.runtime.claude } as const;
      default: return { kind: 'unknown', id } as const;
    }
  }
}
