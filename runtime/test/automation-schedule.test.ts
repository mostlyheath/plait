import assert from 'node:assert/strict';
import { performance } from 'node:perf_hooks';
import { test } from 'node:test';
import {
  ScheduleError,
  defaultCatchUpMs,
  dueOccurrences,
  nextOccurrences,
  validateSchedule,
  type Schedule,
} from '../src/automations/schedule.ts';

const utc = (iso: string): number => Date.parse(iso);

function formatLocal(ms: number, timeZone: string): string {
  const parts = new Intl.DateTimeFormat('en-US', {
    timeZone,
    calendar: 'gregory',
    numberingSystem: 'latn',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    hourCycle: 'h23',
  }).formatToParts(new Date(ms));
  const get = (type: string) => parts.find(p => p.type === type)?.value ?? '';
  return `${get('year')}-${get('month')}-${get('day')}T${get('hour')}:${get('minute')}`;
}

test('validateSchedule accepts valid schedules, normalizes weekly days, and returns a fresh copy', () => {
  const once = validateSchedule({ kind: 'once', at: '2026-06-15T09:30', timeZone: 'UTC' });
  assert.deepEqual(once, { kind: 'once', at: '2026-06-15T09:30', timeZone: 'UTC' });

  const leapOnce = validateSchedule({ kind: 'once', at: '2024-02-29T23:59', timeZone: 'America/Chicago' });
  assert.deepEqual(leapOnce, { kind: 'once', at: '2024-02-29T23:59', timeZone: 'America/Chicago' });

  assert.deepEqual(
    validateSchedule({ kind: 'hourly', minute: 0, timeZone: 'Asia/Kolkata' }),
    { kind: 'hourly', minute: 0, timeZone: 'Asia/Kolkata' },
  );
  assert.deepEqual(
    validateSchedule({ kind: 'daily', time: '00:00', timeZone: 'America/Chicago' }),
    { kind: 'daily', time: '00:00', timeZone: 'America/Chicago' },
  );
  assert.deepEqual(
    validateSchedule({ kind: 'weekdays', time: '09:05', timeZone: 'Europe/London' }),
    { kind: 'weekdays', time: '09:05', timeZone: 'Europe/London' },
  );

  const sourceDays = [5, 1, 3];
  const weekly = validateSchedule({ kind: 'weekly', days: sourceDays, time: '14:00', timeZone: 'UTC' });
  assert.deepEqual(weekly, { kind: 'weekly', days: [1, 3, 5], time: '14:00', timeZone: 'UTC' });
  sourceDays.push(6);
  assert.deepEqual((weekly as Extract<Schedule, { kind: 'weekly' }>).days, [1, 3, 5]);
});

test('validateSchedule rejects unknown kinds, extra keys, bad zones, bad times, and invalid days', () => {
  const invalid: unknown[] = [
    null,
    undefined,
    'daily',
    [],
    { kind: 'monthly', time: '09:00', timeZone: 'UTC' },
    { kind: 'daily', time: '09:00', timeZone: 'UTC', extra: true },
    { kind: 'hourly', minute: 15, timeZone: 'UTC', time: '09:00' },
    { kind: 'once', at: '2026-06-15T09:30', timeZone: 'UTC', days: [1] },
    { kind: 'daily', time: '09:00', timeZone: 'Mars/Olympus_Mons' },
    { kind: 'daily', time: '09:00', timeZone: '' },
    { kind: 'daily', time: '09:00', timeZone: '+05:00' },
    { kind: 'daily', time: '09:00' },
    { kind: 'daily', time: '9:00', timeZone: 'UTC' },
    { kind: 'daily', time: '24:00', timeZone: 'UTC' },
    { kind: 'daily', time: '12:60', timeZone: 'UTC' },
    { kind: 'daily', time: '12:00:00', timeZone: 'UTC' },
    { kind: 'once', at: '2026-02-29T09:00', timeZone: 'UTC' },
    { kind: 'once', at: '2026-04-31T09:00', timeZone: 'UTC' },
    { kind: 'once', at: '2026-06-15T09:00Z', timeZone: 'UTC' },
    { kind: 'once', at: '2026-06-15 09:00', timeZone: 'UTC' },
    { kind: 'hourly', minute: -1, timeZone: 'UTC' },
    { kind: 'hourly', minute: 60, timeZone: 'UTC' },
    { kind: 'hourly', minute: 15.5, timeZone: 'UTC' },
    { kind: 'hourly', minute: Number.NaN, timeZone: 'UTC' },
    { kind: 'hourly', minute: '15', timeZone: 'UTC' },
    { kind: 'weekly', days: [], time: '09:00', timeZone: 'UTC' },
    { kind: 'weekly', days: [1, 1], time: '09:00', timeZone: 'UTC' },
    { kind: 'weekly', days: [7], time: '09:00', timeZone: 'UTC' },
    { kind: 'weekly', days: [-1], time: '09:00', timeZone: 'UTC' },
    { kind: 'weekly', days: [1.5], time: '09:00', timeZone: 'UTC' },
    { kind: 'weekly', days: '1', time: '09:00', timeZone: 'UTC' },
  ];
  for (const input of invalid) {
    assert.throws(() => validateSchedule(input), ScheduleError, JSON.stringify(input));
  }
});

test('nextOccurrences returns the next three runs for each kind and respects strictly-after semantics', () => {
  const once: Schedule = { kind: 'once', at: '2026-06-15T09:30', timeZone: 'UTC' };
  assert.deepEqual(nextOccurrences(once, utc('2026-06-15T09:29:59.999Z'), 3), [utc('2026-06-15T09:30:00Z')]);
  assert.deepEqual(nextOccurrences(once, utc('2026-06-15T09:30:00Z'), 3), []);
  assert.deepEqual(nextOccurrences(once, utc('2026-06-15T10:00:00Z'), 3), []);

  const hourly: Schedule = { kind: 'hourly', minute: 15, timeZone: 'UTC' };
  assert.deepEqual(nextOccurrences(hourly, utc('2026-06-15T09:15:00Z'), 3), [
    utc('2026-06-15T10:15:00Z'),
    utc('2026-06-15T11:15:00Z'),
    utc('2026-06-15T12:15:00Z'),
  ]);
  assert.deepEqual(nextOccurrences(hourly, utc('2026-06-15T09:15:00Z'), 0), []);

  const daily: Schedule = { kind: 'daily', time: '08:30', timeZone: 'UTC' };
  assert.deepEqual(nextOccurrences(daily, utc('2026-06-15T08:30:00Z'), 3), [
    utc('2026-06-16T08:30:00Z'),
    utc('2026-06-17T08:30:00Z'),
    utc('2026-06-18T08:30:00Z'),
  ]);

  // 2026-03-06 is Friday; next three weekday runs cross Saturday and Sunday.
  const weekdays: Schedule = { kind: 'weekdays', time: '09:00', timeZone: 'UTC' };
  assert.deepEqual(nextOccurrences(weekdays, utc('2026-03-06T08:00:00Z'), 3), [
    utc('2026-03-06T09:00:00Z'),
    utc('2026-03-09T09:00:00Z'),
    utc('2026-03-10T09:00:00Z'),
  ]);

  // 2026-03-08 is Sunday; weekly on Mon/Wed/Fri yields Mar 9, Mar 11, Mar 13.
  const weekly: Schedule = { kind: 'weekly', days: [5, 1, 3], time: '14:00', timeZone: 'UTC' };
  assert.deepEqual(nextOccurrences(weekly, utc('2026-03-08T00:00:00Z'), 3), [
    utc('2026-03-09T14:00:00Z'),
    utc('2026-03-11T14:00:00Z'),
    utc('2026-03-13T14:00:00Z'),
  ]);

  assert.throws(() => nextOccurrences(daily, utc('2026-06-15T00:00:00Z'), 101), ScheduleError);
  assert.throws(() => nextOccurrences(daily, utc('2026-06-15T00:00:00Z'), -1), ScheduleError);
});

test('America/Chicago spring-forward and fall-back days follow the DST rules', () => {
  const daily0230: Schedule = { kind: 'daily', time: '02:30', timeZone: 'America/Chicago' };

  // Spring forward: 2026-03-08 jumps 02:00 CST (-06:00) -> 03:00 CDT (-05:00).
  // Missing 02:30 shifts forward by the 1-hour gap to 03:30 CDT (08:30Z).
  const springRuns = nextOccurrences(daily0230, utc('2026-03-07T00:00:00Z'), 3);
  assert.deepEqual(springRuns, [
    utc('2026-03-07T08:30:00Z'), // 2026-03-07 02:30 CST
    utc('2026-03-08T08:30:00Z'), // 2026-03-08 03:30 CDT (02:30 + 1h gap)
    utc('2026-03-09T07:30:00Z'), // 2026-03-09 02:30 CDT
  ]);
  assert.deepEqual(
    springRuns.map(t => formatLocal(t, 'America/Chicago')),
    ['2026-03-07T02:30', '2026-03-08T03:30', '2026-03-09T02:30'],
  );

  // Fall back: 2026-11-01 repeats 01:00..01:59 (CDT -05:00 -> CST -06:00).
  // Daily 02:30 runs once in CST (25 real hours after Saturday's 02:30 CDT).
  const fall0230Runs = nextOccurrences(daily0230, utc('2026-10-31T00:00:00Z'), 3);
  assert.deepEqual(fall0230Runs, [
    utc('2026-10-31T07:30:00Z'), // 2026-10-31 02:30 CDT
    utc('2026-11-01T08:30:00Z'), // 2026-11-01 02:30 CST
    utc('2026-11-02T08:30:00Z'), // 2026-11-02 02:30 CST
  ]);
  assert.equal(fall0230Runs[1] - fall0230Runs[0], 25 * 3_600_000);

  // Daily 01:30 falls inside the repeated hour and runs once, at the earlier (CDT) instant.
  const daily0130: Schedule = { kind: 'daily', time: '01:30', timeZone: 'America/Chicago' };
  const fall0130Runs = nextOccurrences(daily0130, utc('2026-10-31T00:00:00Z'), 3);
  assert.deepEqual(fall0130Runs, [
    utc('2026-10-31T06:30:00Z'), // 2026-10-31 01:30 CDT
    utc('2026-11-01T06:30:00Z'), // 2026-11-01 01:30 CDT (first occurrence, not 07:30Z CST)
    utc('2026-11-02T07:30:00Z'), // 2026-11-02 01:30 CST
  ]);
  // Immediately after the earlier 01:30 CDT, the next run is the following day, not 01:30 CST.
  assert.deepEqual(nextOccurrences(daily0130, utc('2026-11-01T06:30:00Z'), 1), [utc('2026-11-02T07:30:00Z')]);

  // Positive-offset fall back (Europe/Berlin, 2026-10-25): 02:30 runs once at the earlier CEST (+02:00) instant.
  const berlin0230: Schedule = { kind: 'daily', time: '02:30', timeZone: 'Europe/Berlin' };
  assert.deepEqual(nextOccurrences(berlin0230, utc('2026-10-24T23:00:00Z'), 2), [
    utc('2026-10-25T00:30:00Z'), // 2026-10-25 02:30 CEST (first occurrence, not 01:30Z CET)
    utc('2026-10-26T01:30:00Z'), // 2026-10-26 02:30 CET
  ]);

  // Hourly runs every real hour across both transitions without skipping or doubling.
  const hourly30: Schedule = { kind: 'hourly', minute: 30, timeZone: 'America/Chicago' };
  assert.deepEqual(nextOccurrences(hourly30, utc('2026-03-08T06:30:00Z'), 3), [
    utc('2026-03-08T07:30:00Z'), // 01:30 CST
    utc('2026-03-08T08:30:00Z'), // 03:30 CDT
    utc('2026-03-08T09:30:00Z'), // 04:30 CDT
  ]);
  assert.deepEqual(nextOccurrences(hourly30, utc('2026-11-01T05:30:00Z'), 3), [
    utc('2026-11-01T06:30:00Z'), // 01:30 CDT
    utc('2026-11-01T07:30:00Z'), // 01:30 CST
    utc('2026-11-01T08:30:00Z'), // 02:30 CST
  ]);
});

test('hourly lands on local minute M in Asia/Kolkata (+05:30) and Australia/Lord_Howe (30-min DST)', () => {
  const kolkata: Schedule = { kind: 'hourly', minute: 15, timeZone: 'Asia/Kolkata' };
  const kolkataRuns = nextOccurrences(kolkata, utc('2026-06-15T00:00:00Z'), 3);
  assert.deepEqual(kolkataRuns, [
    utc('2026-06-15T00:45:00Z'),
    utc('2026-06-15T01:45:00Z'),
    utc('2026-06-15T02:45:00Z'),
  ]);
  assert.deepEqual(
    kolkataRuns.map(t => formatLocal(t, 'Asia/Kolkata')),
    ['2026-06-15T06:15', '2026-06-15T07:15', '2026-06-15T08:15'],
  );

  // Australia/Lord_Howe spring forward: 2026-10-04 02:00 LHST (+10:30) -> 02:30 LHDT (+11:00) at 15:30Z.
  const lordHowe15: Schedule = { kind: 'hourly', minute: 15, timeZone: 'Australia/Lord_Howe' };
  const springRuns = nextOccurrences(lordHowe15, utc('2026-10-03T14:00:00Z'), 3);
  assert.deepEqual(springRuns, [
    utc('2026-10-03T14:45:00Z'), // 2026-10-04 01:15 LHST
    utc('2026-10-03T16:15:00Z'), // 2026-10-04 03:15 LHDT
    utc('2026-10-03T17:15:00Z'), // 2026-10-04 04:15 LHDT
  ]);
  assert.deepEqual(
    springRuns.map(t => formatLocal(t, 'Australia/Lord_Howe')),
    ['2026-10-04T01:15', '2026-10-04T03:15', '2026-10-04T04:15'],
  );

  // Australia/Lord_Howe fall back: 2026-04-05 02:00 LHDT (+11:00) -> 01:30 LHST (+10:30) at 15:00Z.
  const fallRuns = nextOccurrences(lordHowe15, utc('2026-04-04T14:00:00Z'), 3);
  assert.deepEqual(fallRuns, [
    utc('2026-04-04T14:15:00Z'), // 2026-04-05 01:15 LHDT
    utc('2026-04-04T15:45:00Z'), // 2026-04-05 02:15 LHST
    utc('2026-04-04T16:45:00Z'), // 2026-04-05 03:15 LHST
  ]);
  assert.deepEqual(
    fallRuns.map(t => formatLocal(t, 'Australia/Lord_Howe')),
    ['2026-04-05T01:15', '2026-04-05T02:15', '2026-04-05T03:15'],
  );

  // Verify dueOccurrences count agrees with stepped hourly runs across a Lord Howe transition.
  const lordHowe45: Schedule = { kind: 'hourly', minute: 45, timeZone: 'Australia/Lord_Howe' };
  const due = dueOccurrences(lordHowe45, utc('2026-10-03T14:00:00Z'), utc('2026-10-03T17:00:00Z'), 30 * 60_000);
  assert.equal(due.run, utc('2026-10-03T16:45:00Z'));
  assert.deepEqual(due.missed, [
    utc('2026-10-03T14:15:00Z'),
    utc('2026-10-03T15:15:00Z'),
    utc('2026-10-03T15:45:00Z'),
  ]);
  assert.equal(due.missedTotal, 3);
});

test('defaultCatchUpMs returns 30 minutes for hourly and 12 hours for all other kinds', () => {
  assert.equal(defaultCatchUpMs({ kind: 'hourly', minute: 0, timeZone: 'UTC' }), 30 * 60_000);
  assert.equal(defaultCatchUpMs({ kind: 'daily', time: '09:00', timeZone: 'UTC' }), 12 * 3_600_000);
  assert.equal(defaultCatchUpMs({ kind: 'weekdays', time: '09:00', timeZone: 'UTC' }), 12 * 3_600_000);
  assert.equal(defaultCatchUpMs({ kind: 'weekly', days: [1], time: '09:00', timeZone: 'UTC' }), 12 * 3_600_000);
  assert.equal(defaultCatchUpMs({ kind: 'once', at: '2026-06-15T09:00', timeZone: 'UTC' }), 12 * 3_600_000);
});

test('dueOccurrences handles nothing due, single due inside/outside window, bursts, and once schedules', () => {
  const daily: Schedule = { kind: 'daily', time: '09:00', timeZone: 'UTC' };
  const windowMs = defaultCatchUpMs(daily);

  // Nothing due between 09:00 and 10:00 on the same day.
  assert.deepEqual(
    dueOccurrences(daily, utc('2026-06-15T09:00:00Z'), utc('2026-06-15T10:00:00Z'), windowMs),
    { run: null, missed: [], missedTotal: 0, next: utc('2026-06-16T09:00:00Z') },
  );

  // One due inside window (exact boundary now - occurrence === catchUpMs still runs).
  assert.deepEqual(
    dueOccurrences(daily, utc('2026-06-15T09:00:00Z'), utc('2026-06-16T21:00:00Z'), windowMs),
    { run: utc('2026-06-16T09:00:00Z'), missed: [], missedTotal: 0, next: utc('2026-06-17T09:00:00Z') },
  );

  // One due outside window is recorded as missed.
  assert.deepEqual(
    dueOccurrences(daily, utc('2026-06-15T09:00:00Z'), utc('2026-06-16T21:00:01Z'), windowMs),
    { run: null, missed: [utc('2026-06-16T09:00:00Z')], missedTotal: 1, next: utc('2026-06-17T09:00:00Z') },
  );

  // Many due: 150 days elapsed, latest inside window runs, missed capped at latest 100, missedTotal = 149.
  const manyInside = dueOccurrences(daily, utc('2026-01-01T09:00:00Z'), utc('2026-05-31T10:00:00Z'), windowMs);
  assert.equal(manyInside.run, utc('2026-05-31T09:00:00Z'));
  assert.equal(manyInside.missedTotal, 149);
  assert.equal(manyInside.missed.length, 100);
  assert.equal(manyInside.missed[0], utc('2026-02-20T09:00:00Z'));
  assert.equal(manyInside.missed[99], utc('2026-05-30T09:00:00Z'));
  assert.equal(manyInside.next, utc('2026-06-01T09:00:00Z'));

  // Many due with latest outside window: run is null, missed capped at latest 100 (including May 31), missedTotal = 150.
  const manyOutside = dueOccurrences(daily, utc('2026-01-01T09:00:00Z'), utc('2026-05-31T22:00:00Z'), windowMs);
  assert.equal(manyOutside.run, null);
  assert.equal(manyOutside.missedTotal, 150);
  assert.equal(manyOutside.missed.length, 100);
  assert.equal(manyOutside.missed[0], utc('2026-02-21T09:00:00Z'));
  assert.equal(manyOutside.missed[99], utc('2026-05-31T09:00:00Z'));

  // Once schedule: future -> due inside -> due outside -> already finished.
  const once: Schedule = { kind: 'once', at: '2026-06-15T09:00', timeZone: 'UTC' };
  assert.deepEqual(
    dueOccurrences(once, utc('2026-06-15T08:00:00Z'), utc('2026-06-15T08:30:00Z'), windowMs),
    { run: null, missed: [], missedTotal: 0, next: utc('2026-06-15T09:00:00Z') },
  );
  assert.deepEqual(
    dueOccurrences(once, utc('2026-06-15T08:00:00Z'), utc('2026-06-15T10:00:00Z'), windowMs),
    { run: utc('2026-06-15T09:00:00Z'), missed: [], missedTotal: 0, next: null },
  );
  assert.deepEqual(
    dueOccurrences(once, utc('2026-06-15T08:00:00Z'), utc('2026-06-16T00:00:00Z'), windowMs),
    { run: null, missed: [utc('2026-06-15T09:00:00Z')], missedTotal: 1, next: null },
  );
  assert.deepEqual(
    dueOccurrences(once, utc('2026-06-15T09:00:00Z'), utc('2026-06-15T10:00:00Z'), windowMs),
    { run: null, missed: [], missedTotal: 0, next: null },
  );
});

test('dueOccurrences completes a year-long hourly gap quickly with exact missed count', () => {
  const hourly: Schedule = { kind: 'hourly', minute: 15, timeZone: 'America/Chicago' };
  const since = utc('2026-01-01T00:15:00Z');
  const now = utc('2027-01-01T00:25:00Z'); // 365 days = 8760 hourly occurrences later, latest inside 30m window

  const start = performance.now();
  const due = dueOccurrences(hourly, since, now, defaultCatchUpMs(hourly));
  const elapsedMs = performance.now() - start;

  assert.ok(elapsedMs < 50, `Expected year-long hourly dueOccurrences under 50 ms, took ${elapsedMs.toFixed(2)} ms`);
  assert.equal(due.run, utc('2027-01-01T00:15:00Z'));
  assert.equal(due.missedTotal, 8759);
  assert.equal(due.missed.length, 100);
  assert.equal(due.missed[0], utc('2026-12-27T20:15:00Z'));
  assert.equal(due.missed[99], utc('2026-12-31T23:15:00Z'));
  assert.equal(due.next, utc('2027-01-01T01:15:00Z'));
});
