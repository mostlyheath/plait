import { fail } from '../protocol.ts';
import { prepareCommand } from './macos.ts';

export function commandPlatform(platform: NodeJS.Platform = process.platform) {
  switch (platform) {
    case 'darwin':
      return { scratchPrefix: '/private/tmp/plait-command-', prepare: prepareCommand };
    default:
      // Missing containment is a denial, never permission to use an ordinary shell.
      fail('permission_denied', 'Commands require the tested macOS filesystem sandbox. Linux command containment is not enabled.');
  }
}

export const LINUX_SANDBOX_UNAVAILABLE = 'The command sandbox is not available on Linux yet. See docs/LINUX-SANDBOX.md.';

export function trustedPlatform(platform: NodeJS.Platform = process.platform) {
  if (platform !== 'darwin' && platform !== 'linux')
    fail('permission_denied', 'Trusted commands are not enabled on this platform.');
}

export function sandboxedCommandPlatform(platform: NodeJS.Platform = process.platform) {
  if (platform === 'linux') fail('invalid_request', LINUX_SANDBOX_UNAVAILABLE);
  return commandPlatform(platform);
}
