import { access, writeFile } from 'node:fs/promises';
import { existsSync, readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { createModels } from '@earendil-works/pi-ai/models';
import { fauxProvider, fauxAssistantMessage, fauxText, fauxToolCall } from '@earendil-works/pi-ai/providers/faux';
import type { FauxResponseStep } from '@earendil-works/pi-ai/providers/faux';
import type { Models } from '@earendil-works/pi-ai';
import type { ConversationRetryPolicy } from '@earendil-works/pi-durable';
import { loadMcp } from '../../src/mcp/config.ts';

export type ScriptedStep = {
  text?: string;
  tool?: { name: string; args: Record<string, string | number>; id: string };
  stopReason?: 'error';
  errorMessage?: string;
  gate?: string;
  pid_file?: string;
};

export type TestScript = {
  steps: ScriptedStep[];
  tokensPerSecond?: number;
  retry?: ConversationRetryPolicy;
  maxTurns?: number;
};

function readCursor(cursorPath: string): number {
  if (!existsSync(cursorPath)) return 0;
  try {
    const parsed = JSON.parse(readFileSync(cursorPath, 'utf8')) as { index?: number };
    return typeof parsed.index === 'number' && Number.isSafeInteger(parsed.index) && parsed.index >= 0 ? parsed.index : 0;
  } catch {
    return 0;
  }
}

function writeCursor(cursorPath: string, index: number): void {
  writeFileSync(cursorPath, JSON.stringify({ index }));
}

async function waitForGate(gate: string, signal?: AbortSignal): Promise<void> {
  while (!signal?.aborted) {
    try {
      await access(gate);
      return;
    } catch {
      await new Promise(resolve => setTimeout(resolve, 25));
    }
  }
}

function buildStep(step: ScriptedStep, cursorPath: string, nextIndex: number): FauxResponseStep {
  return async (_context, options?: { signal?: AbortSignal }) => {
    writeCursor(cursorPath, nextIndex);
    if (step.pid_file) await writeFile(step.pid_file, String(process.pid));
    if (step.gate) await waitForGate(step.gate, options?.signal);
    if (step.stopReason === 'error') {
      return fauxAssistantMessage([], { stopReason: 'error', errorMessage: step.errorMessage ?? 'SCRIPTED_ERROR' });
    }
    if (step.tool) {
      const content = [
        ...(step.text ? [fauxText(step.text)] : []),
        fauxToolCall(step.tool.name, step.tool.args, { id: step.tool.id }),
      ];
      return fauxAssistantMessage(content, { stopReason: 'toolUse' });
    }
    return fauxAssistantMessage([fauxText(step.text ?? 'Scripted test response.')]);
  };
}

/** Test-only provider loader for detached Pi runners under PLAIT_TEST_SCENARIO. */
export async function loadTestProvider(store: string, scenario: string): Promise<{
  models: Models;
  retry?: ConversationRetryPolicy;
  maxTurns?: number;
  resources: (checkout: string, trusted: boolean, options?: { protect?: string[]; agentDir?: string; describe?: boolean }) => ReturnType<typeof import('../../src/skills.ts').discover>;
}> {
  const models = createModels();
  const scriptPath = process.env.PLAIT_TEST_SCRIPT ?? join(store, 'pi-test-script.json');
  const cursorPath = join(store, 'pi-test-cursor.json');
  let responses: FauxResponseStep[] = [];
  let tokensPerSecond = 1e8;
  let retry: ConversationRetryPolicy | undefined;
  let maxTurns: number | undefined;

  if (scenario === 'pi-script' && existsSync(scriptPath)) {
    const script = JSON.parse(readFileSync(scriptPath, 'utf8')) as TestScript;
    tokensPerSecond = script.tokensPerSecond ?? 1e8;
    retry = script.retry;
    maxTurns = script.maxTurns;
    const startIndex = readCursor(cursorPath);
    responses = script.steps.slice(startIndex).map((step, offset) => buildStep(step, cursorPath, startIndex + offset + 1));
  } else if (scenario === 'edit') {
    responses = [
      fauxAssistantMessage([fauxToolCall('read', { path: 'notes.txt' }, { id: 'read-1' })], { stopReason: 'toolUse' }),
      fauxAssistantMessage([fauxToolCall('edit', { path: 'notes.txt', old_text: 'before', new_text: 'after' }, { id: 'edit-1' })], { stopReason: 'toolUse' }),
      fauxAssistantMessage([fauxText('Edited notes.txt.')]),
    ];
  } else {
    responses = [fauxAssistantMessage([fauxText('Scripted detached Pi response.')])];
  }

  responses.push(...Array.from({ length: 8 }, () => fauxAssistantMessage([fauxText('Scripted test response.')])));
  const faux = fauxProvider({
    tokensPerSecond,
    tokenSize: tokensPerSecond >= 1e6 ? { min: 2048, max: 2048 } : { min: 1, max: 1 },
  });
  faux.setResponses(responses);
  models.setProvider(faux.provider);

  return {
    models,
    retry,
    maxTurns,
    async resources(checkout: string, trusted: boolean, options?: { describe?: boolean }) {
      const mcp = await loadMcp(checkout, trusted, join(store, 'test-pi'), true, options?.describe ?? false);
      return { skills: [], templates: [], problems: mcp.problems, mcpServers: mcp.servers, mcpEntries: mcp.entries };
    },
  };
}
