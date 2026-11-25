// A runner that fails to start only after the supervisor's launch deadline has passed.
setTimeout(() => { console.error('slow runner: listen failed'); process.exitCode = 1; }, Number(process.env.PLAIT_TEST_FAIL_AFTER_MS ?? 300));
