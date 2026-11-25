// Schedule validation, next-occurrence calculation, and missed-run accounting.
//
// Daylight-saving rules:
// - Spring-forward gap (a local time that does not exist, e.g. 02:30 on a US spring-forward day):
//   shifted forward by the gap length (so 02:30 becomes 03:30 local for a one-hour gap, and
//   02:15 becomes 02:45 local for a 30-minute gap such as Australia/Lord_Howe).
// - Fall-back overlap (a local time that occurs twice): runs once, at the first (earlier) instant.
// - 'hourly' runs at local minute M of every real hour (by elapsed UTC time), so integer-hour DST
//   transitions neither skip nor double a run; zones with 30/45-minute offsets (and 30-minute DST
//   such as Australia/Lord_Howe) still land on local minute M.

export type Weekday = 0 | 1 | 2 | 3 | 4 | 5 | 6; // 0 = Sunday
export type Schedule =
  | { kind: 'once'; at: string; timeZone: string }            // at = local 'YYYY-MM-DDTHH:mm'
  | { kind: 'hourly'; minute: number; timeZone: string }       // minute 0..59
  | { kind: 'daily'; time: string; timeZone: string }          // time = 'HH:mm'
  | { kind: 'weekdays'; time: string; timeZone: string }       // Monday..Friday
  | { kind: 'weekly'; days: Weekday[]; time: string; timeZone: string }; // days non-empty, unique

export class ScheduleError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'ScheduleError';
  }
}

export type Due = {
  run: number | null;     // the one occurrence to start now (the latest due one, if inside the window), else null
  missed: number[];       // other due occurrences recorded as missed, ascending, at most the latest 100
  missedTotal: number;    // how many were missed in total (may exceed missed.length)
  next: number | null;    // the first occurrence strictly after now, or null (a finished 'once')
};

const MINUTE_MS = 60_000;
const HOUR_MS = 60 * MINUTE_MS;
const DAY_MS = 24 * HOUR_MS;
const DST_PROBE_MS = 36 * HOUR_MS; // Spans any real zone offset and transition with margin to spare.
const HOURLY_CHUNK_MS = 28 * DAY_MS; // Shorter than the gap between real DST transitions.
const MAX_COUNT = 100;
const MAX_MISSED = 100;
const FORMATTER_CACHE_LIMIT = 256;

const ALL_DAYS: readonly Weekday[] = [0, 1, 2, 3, 4, 5, 6];
const WEEKDAYS: readonly Weekday[] = [1, 2, 3, 4, 5];

const ONCE_RE = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2})$/;
const TIME_RE = /^(\d{2}):(\d{2})$/;

const formatters = new Map<string, Intl.DateTimeFormat>();

type LocalParts = {
  year: number;
  month: number;
  day: number;
  hour: number;
  minute: number;
  second: number;
};

function mod(value: number, divisor: number): number {
  return ((value % divisor) + divisor) % divisor;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}

function assertExactKeys(obj: Record<string, unknown>, allowed: readonly string[]): void {
  for (const key of Object.keys(obj)) {
    if (!allowed.includes(key)) throw new ScheduleError(`Unexpected schedule field "${key}".`);
  }
}

function getFormatter(timeZone: string): Intl.DateTimeFormat {
  let formatter = formatters.get(timeZone);
  if (formatter) return formatter;
  try {
    formatter = new Intl.DateTimeFormat('en-US', {
      timeZone,
      calendar: 'gregory',
      numberingSystem: 'latn',
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit',
      hourCycle: 'h23',
    });
  } catch {
    throw new ScheduleError(`Invalid IANA time zone "${timeZone}".`);
  }
  if (formatters.size >= FORMATTER_CACHE_LIMIT) formatters.clear();
  formatters.set(timeZone, formatter);
  return formatter;
}

function validateTimeZone(value: unknown): string {
  // Reject non-strings and empty values up front because Intl treats undefined as the host zone.
  if (typeof value !== 'string' || value.length === 0 || value.length > 128 || /[\s\p{C}]/u.test(value)) {
    throw new ScheduleError('Schedule timeZone must be a valid IANA time zone.');
  }
  // Reject bare numeric offsets (e.g. "+02:00") that newer ICU accepts in Intl.DateTimeFormat.
  if (/^[+-]/.test(value)) {
    throw new ScheduleError(`Invalid IANA time zone "${value}".`);
  }
  getFormatter(value);
  return value;
}

function daysInMonth(year: number, month: number): number {
  if (month === 2) {
    const leap = (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0;
    return leap ? 29 : 28;
  }
  return month === 4 || month === 6 || month === 9 || month === 11 ? 30 : 31;
}

function parseTime(value: unknown): { hour: number; minute: number; text: string } {
  if (typeof value !== 'string') throw new ScheduleError('Schedule time must be "HH:mm".');
  const match = TIME_RE.exec(value);
  if (!match) throw new ScheduleError(`Invalid schedule time "${value}"; expected "HH:mm".`);
  const hour = Number(match[1]);
  const minute = Number(match[2]);
  if (hour > 23 || minute > 59) throw new ScheduleError(`Out-of-range schedule time "${value}".`);
  return { hour, minute, text: value };
}

function parseOnceAt(value: unknown): { year: number; month: number; day: number; hour: number; minute: number; text: string } {
  if (typeof value !== 'string') throw new ScheduleError('Schedule "at" must be "YYYY-MM-DDTHH:mm".');
  const match = ONCE_RE.exec(value);
  if (!match) throw new ScheduleError(`Invalid "at" timestamp "${value}"; expected "YYYY-MM-DDTHH:mm".`);
  const year = Number(match[1]);
  const month = Number(match[2]);
  const day = Number(match[3]);
  const hour = Number(match[4]);
  const minute = Number(match[5]);
  if (year < 1 || month < 1 || month > 12 || day < 1 || day > daysInMonth(year, month) || hour > 23 || minute > 59) {
    throw new ScheduleError(`Out-of-range "at" timestamp "${value}".`);
  }
  return { year, month, day, hour, minute, text: value };
}

function validateMinute(value: unknown): number {
  if (typeof value !== 'number' || !Number.isInteger(value) || value < 0 || value > 59) {
    throw new ScheduleError('Schedule minute must be an integer from 0 to 59.');
  }
  return value === 0 ? 0 : value;
}

function validateDays(value: unknown): Weekday[] {
  if (!Array.isArray(value) || value.length === 0) {
    throw new ScheduleError('Weekly schedule days must be a non-empty array.');
  }
  const seen = new Set<number>();
  const normalized: Weekday[] = [];
  for (const entry of value) {
    if (typeof entry !== 'number' || !Number.isInteger(entry) || entry < 0 || entry > 6) {
      throw new ScheduleError('Weekly schedule days must be integers from 0 (Sunday) to 6 (Saturday).');
    }
    const day = (entry === 0 ? 0 : entry) as Weekday;
    if (seen.has(day)) throw new ScheduleError(`Duplicate weekday ${day} in weekly schedule.`);
    seen.add(day);
    normalized.push(day);
  }
  normalized.sort((a, b) => a - b);
  return normalized;
}

// Validates untrusted input (from the protocol) and returns a normalized copy (weekly days sorted).
export function validateSchedule(input: unknown): Schedule {
  if (!isRecord(input)) throw new ScheduleError('Schedule must be an object.');
  const kind = input.kind;
  switch (kind) {
    case 'once': {
      assertExactKeys(input, ['kind', 'at', 'timeZone']);
      const at = parseOnceAt(input.at).text;
      const timeZone = validateTimeZone(input.timeZone);
      return { kind: 'once', at, timeZone };
    }
    case 'hourly': {
      assertExactKeys(input, ['kind', 'minute', 'timeZone']);
      const minute = validateMinute(input.minute);
      const timeZone = validateTimeZone(input.timeZone);
      return { kind: 'hourly', minute, timeZone };
    }
    case 'daily': {
      assertExactKeys(input, ['kind', 'time', 'timeZone']);
      const time = parseTime(input.time).text;
      const timeZone = validateTimeZone(input.timeZone);
      return { kind: 'daily', time, timeZone };
    }
    case 'weekdays': {
      assertExactKeys(input, ['kind', 'time', 'timeZone']);
      const time = parseTime(input.time).text;
      const timeZone = validateTimeZone(input.timeZone);
      return { kind: 'weekdays', time, timeZone };
    }
    case 'weekly': {
      assertExactKeys(input, ['kind', 'days', 'time', 'timeZone']);
      const days = validateDays(input.days);
      const time = parseTime(input.time).text;
      const timeZone = validateTimeZone(input.timeZone);
      return { kind: 'weekly', days, time, timeZone };
    }
    default:
      throw new ScheduleError('Unknown schedule kind.');
  }
}

function utcMs(year: number, month: number, day: number, hour: number, minute: number, second = 0): number {
  const ms = Date.UTC(year, month - 1, day, hour, minute, second);
  if (year >= 0 && year < 100) {
    // Date.UTC maps years 0..99 into 1900..1999; restore the literal year.
    const date = new Date(ms);
    date.setUTCFullYear(year);
    return date.getTime();
  }
  return ms;
}

function localParts(ms: number, timeZone: string): LocalParts {
  const parts = getFormatter(timeZone).formatToParts(new Date(ms));
  let year = 0;
  let month = 0;
  let day = 0;
  let hour = 0;
  let minute = 0;
  let second = 0;
  for (const part of parts) {
    switch (part.type) {
      case 'year': year = Number(part.value); break;
      case 'month': month = Number(part.value); break;
      case 'day': day = Number(part.value); break;
      case 'hour': hour = Number(part.value); break;
      case 'minute': minute = Number(part.value); break;
      case 'second': second = Number(part.value); break;
    }
  }
  // Guard against ICU variants that format midnight as 24.
  if (hour === 24) hour = 0;
  return { year, month, day, hour, minute, second };
}

function offsetMs(ms: number, timeZone: string): number {
  const wholeSecond = Math.floor(ms / 1000) * 1000;
  const parts = localParts(wholeSecond, timeZone);
  return utcMs(parts.year, parts.month, parts.day, parts.hour, parts.minute, parts.second) - wholeSecond;
}

// Converts a local wall-clock timestamp (encoded as UTC ms) to an epoch instant in timeZone.
function resolveLocalWallClock(targetAsUtc: number, timeZone: string): number {
  const earlierOffset = offsetMs(targetAsUtc - DST_PROBE_MS, timeZone);
  const laterOffset = offsetMs(targetAsUtc + DST_PROBE_MS, timeZone);
  if (earlierOffset === laterOffset) return targetAsUtc - earlierOffset;

  const before = targetAsUtc - earlierOffset;
  if (offsetMs(before, timeZone) === earlierOffset) return before;

  const after = targetAsUtc - laterOffset;
  if (offsetMs(after, timeZone) === laterOffset) return after;

  // Non-existent spring-forward time: shift forward by the gap length (laterOffset - earlierOffset),
  // which simplifies to targetAsUtc - earlierOffset.
  return before;
}

function localMinuteAt(wholeMinuteMs: number, offset: number): number {
  return mod(Math.floor((wholeMinuteMs + offset) / MINUTE_MS), 60);
}

// Finds the earliest whole minute in (lo, hi] whose UTC offset differs from loOffset.
function findTransitionForward(lo: number, hi: number, loOffset: number, timeZone: string): number {
  let lowMin = Math.floor(lo / MINUTE_MS);
  let highMin = Math.floor(hi / MINUTE_MS);
  while (highMin - lowMin > 1) {
    const midMin = Math.floor((lowMin + highMin) / 2);
    if (offsetMs(midMin * MINUTE_MS, timeZone) === loOffset) lowMin = midMin;
    else highMin = midMin;
  }
  return highMin * MINUTE_MS;
}

// Finds the latest whole minute in [lo, hi) whose UTC offset differs from hiOffset.
function findTransitionBackward(lo: number, hi: number, hiOffset: number, timeZone: string): number {
  let lowMin = Math.floor(lo / MINUTE_MS);
  let highMin = Math.floor(hi / MINUTE_MS);
  while (highMin - lowMin > 1) {
    const midMin = Math.floor((lowMin + highMin) / 2);
    if (offsetMs(midMin * MINUTE_MS, timeZone) === hiOffset) highMin = midMin;
    else lowMin = midMin;
  }
  return lowMin * MINUTE_MS;
}

function nextHourly(after: number, minute: number, timeZone: string): number {
  let cursor = Math.floor(after / MINUTE_MS) * MINUTE_MS + MINUTE_MS;
  for (let i = 0; i < 4; i++) {
    const off = offsetMs(cursor, timeZone);
    const delta = mod(minute - localMinuteAt(cursor, off), 60);
    const candidate = cursor + delta * MINUTE_MS;
    if (delta === 0 || offsetMs(candidate, timeZone) === off) return candidate;
    cursor = findTransitionForward(cursor, candidate, off, timeZone);
  }
  throw new ScheduleError('Could not resolve next hourly occurrence.');
}

function prevHourly(atOrBefore: number, minute: number, timeZone: string): number {
  let cursor = Math.floor(atOrBefore / MINUTE_MS) * MINUTE_MS;
  for (let i = 0; i < 4; i++) {
    const off = offsetMs(cursor, timeZone);
    const delta = mod(localMinuteAt(cursor, off) - minute, 60);
    const candidate = cursor - delta * MINUTE_MS;
    if (delta === 0 || offsetMs(candidate, timeZone) === off) return candidate;
    cursor = findTransitionBackward(candidate, cursor, off, timeZone);
  }
  throw new ScheduleError('Could not resolve previous hourly occurrence.');
}

function countHourlyConstantOffset(startMinMs: number, endMinMs: number, offset: number, minute: number): number {
  if (startMinMs > endMinMs) return 0;
  const delta = mod(minute - localMinuteAt(startMinMs, offset), 60);
  const first = startMinMs + delta * MINUTE_MS;
  if (first > endMinMs) return 0;
  return Math.floor((endMinMs - first) / HOUR_MS) + 1;
}

function countHourlySpan(
  startMinMs: number,
  endMinMs: number,
  startOff: number,
  endOff: number,
  minute: number,
  timeZone: string,
): number {
  if (startMinMs > endMinMs) return 0;
  if (startOff === endOff) return countHourlyConstantOffset(startMinMs, endMinMs, startOff, minute);
  const transition = findTransitionForward(startMinMs, endMinMs, startOff, timeZone);
  const before = countHourlyConstantOffset(startMinMs, transition - MINUTE_MS, startOff, minute);
  const transOff = offsetMs(transition, timeZone);
  const after = countHourlySpan(transition, endMinMs, transOff, endOff, minute, timeZone);
  return before + after;
}

function countHourlyInRange(since: number, now: number, minute: number, timeZone: string): number {
  const startMinMs = Math.floor(since / MINUTE_MS) * MINUTE_MS + MINUTE_MS;
  const endMinMs = Math.floor(now / MINUTE_MS) * MINUTE_MS;
  if (startMinMs > endMinMs) return 0;
  let total = 0;
  let segStart = startMinMs;
  let segStartOff = offsetMs(segStart, timeZone);
  while (segStart <= endMinMs) {
    const segEnd = Math.min(segStart + HOURLY_CHUNK_MS, endMinMs);
    const segEndOff = segEnd === segStart ? segStartOff : offsetMs(segEnd, timeZone);
    total += countHourlySpan(segStart, segEnd, segStartOff, segEndOff, minute, timeZone);
    segStart = segEnd + MINUTE_MS;
    if (segStart <= endMinMs) segStartOff = offsetMs(segStart, timeZone);
  }
  return total;
}

function collectLatestHourly(since: number, now: number, minute: number, timeZone: string, limit: number): number[] {
  const last = prevHourly(now, minute, timeZone);
  if (last <= since) return [];
  const lastOff = offsetMs(last, timeZone);
  const probeStart = Math.max(since, last - (limit - 1) * HOUR_MS);
  // Fast path: when no offset transition falls in the trailing window, occurrences are strictly 1 hour apart.
  if (offsetMs(probeStart, timeZone) === lastOff) {
    const latest: number[] = [];
    for (let i = 0; i < limit; i++) {
      const instant = last - i * HOUR_MS;
      if (instant <= since) break;
      latest.push(instant);
    }
    latest.reverse();
    return latest;
  }
  const latest: number[] = [last];
  let cursor = last - MINUTE_MS;
  while (latest.length < limit) {
    const instant = prevHourly(cursor, minute, timeZone);
    if (instant <= since) break;
    latest.push(instant);
    cursor = instant - MINUTE_MS;
  }
  latest.reverse();
  return latest;
}

function dayNumForLocal(ms: number, timeZone: string): number {
  const parts = localParts(ms, timeZone);
  return Math.floor(utcMs(parts.year, parts.month, parts.day, 0, 0, 0) / DAY_MS);
}

// 1970-01-01 (day 0) was a Thursday (4).
function weekdayOf(dayNum: number): Weekday {
  return mod(dayNum + 4, 7) as Weekday;
}

function nextAllowedDayOnOrAfter(dayNum: number, days: readonly Weekday[]): number {
  const current = weekdayOf(dayNum);
  for (const day of days) {
    if (day >= current) return dayNum + (day - current);
  }
  return dayNum + (7 - current + days[0]);
}

function prevAllowedDayOnOrBefore(dayNum: number, days: readonly Weekday[]): number {
  const current = weekdayOf(dayNum);
  for (let i = days.length - 1; i >= 0; i--) {
    if (days[i] <= current) return dayNum - (current - days[i]);
  }
  return dayNum - (current + 7 - days[days.length - 1]);
}

function calendarOccurrenceOnDay(dayNum: number, timeMinutes: number, timeZone: string): number {
  return resolveLocalWallClock(dayNum * DAY_MS + timeMinutes * MINUTE_MS, timeZone);
}

function firstCalendarDayAfter(after: number, days: readonly Weekday[], timeMinutes: number, timeZone: string): number {
  // Start one local day early to stay safe across midnight DST shifts.
  let day = nextAllowedDayOnOrAfter(dayNumForLocal(after, timeZone) - 1, days);
  while (calendarOccurrenceOnDay(day, timeMinutes, timeZone) <= after) {
    day = nextAllowedDayOnOrAfter(day + 1, days);
  }
  return day;
}

function lastCalendarDayAtOrBefore(atOrBefore: number, days: readonly Weekday[], timeMinutes: number, timeZone: string): number {
  let day = prevAllowedDayOnOrBefore(dayNumForLocal(atOrBefore, timeZone) + 1, days);
  while (calendarOccurrenceOnDay(day, timeMinutes, timeZone) > atOrBefore) {
    day = prevAllowedDayOnOrBefore(day - 1, days);
  }
  return day;
}

function countAllowedDays(firstDay: number, lastDay: number, days: readonly Weekday[]): number {
  if (firstDay > lastDay) return 0;
  const span = lastDay - firstDay + 1;
  let count = Math.floor(span / 7) * days.length;
  const remainder = span % 7;
  for (let i = 0; i < remainder; i++) {
    if (days.includes(weekdayOf(firstDay + i))) count++;
  }
  return count;
}

function calendarConfig(schedule: Exclude<Schedule, { kind: 'once' } | { kind: 'hourly' }>): { days: readonly Weekday[]; timeMinutes: number } {
  const { hour, minute } = parseTime(schedule.time);
  const timeMinutes = hour * 60 + minute;
  if (schedule.kind === 'daily') return { days: ALL_DAYS, timeMinutes };
  if (schedule.kind === 'weekdays') return { days: WEEKDAYS, timeMinutes };
  return { days: schedule.days, timeMinutes };
}

function onceInstant(schedule: Extract<Schedule, { kind: 'once' }>): number {
  const { year, month, day, hour, minute } = parseOnceAt(schedule.at);
  return resolveLocalWallClock(utcMs(year, month, day, hour, minute), schedule.timeZone);
}

function assertFiniteTimestamp(value: number, label: string): void {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    throw new ScheduleError(`${label} must be a finite epoch timestamp.`);
  }
}

// Occurrence instants (epoch ms) strictly after `after`, ascending, at most `count` (count 0..100).
export function nextOccurrences(schedule: Schedule, after: number, count: number): number[] {
  const normalized = validateSchedule(schedule);
  assertFiniteTimestamp(after, 'after');
  if (typeof count !== 'number' || !Number.isInteger(count) || count < 0 || count > MAX_COUNT) {
    throw new ScheduleError(`count must be an integer from 0 to ${MAX_COUNT}.`);
  }
  if (count === 0) return [];

  if (normalized.kind === 'once') {
    const instant = onceInstant(normalized);
    return instant > after ? [instant] : [];
  }

  if (normalized.kind === 'hourly') {
    const result: number[] = [];
    let cursor = after;
    while (result.length < count) {
      const instant = nextHourly(cursor, normalized.minute, normalized.timeZone);
      result.push(instant);
      cursor = instant;
    }
    return result;
  }

  const { days, timeMinutes } = calendarConfig(normalized);
  const result: number[] = [];
  let day = firstCalendarDayAfter(after, days, timeMinutes, normalized.timeZone);
  while (result.length < count) {
    result.push(calendarOccurrenceOnDay(day, timeMinutes, normalized.timeZone));
    day = nextAllowedDayOnOrAfter(day + 1, days);
  }
  return result;
}

// Default catch-up window: 30 minutes for hourly; 12 hours for daily, weekdays, weekly and once.
export function defaultCatchUpMs(schedule: Schedule): number {
  const normalized = validateSchedule(schedule);
  return normalized.kind === 'hourly' ? 30 * MINUTE_MS : 12 * HOUR_MS;
}

function buildDue(totalDue: number, latestAscending: number[], now: number, catchUpMs: number, next: number | null): Due {
  if (totalDue === 0 || latestAscending.length === 0) {
    return { run: null, missed: [], missedTotal: 0, next };
  }
  const latest = latestAscending[latestAscending.length - 1];
  if (now - latest <= catchUpMs) {
    return {
      run: latest,
      missed: latestAscending.slice(0, -1),
      missedTotal: totalDue - 1,
      next,
    };
  }
  return {
    run: null,
    missed: latestAscending.slice(-MAX_MISSED),
    missedTotal: totalDue,
    next,
  };
}

// Occurrences in (since, now] are due. Only the latest due occurrence may run, and only if
// now - occurrence <= catchUpMs; every other due occurrence is missed. Never returns a burst.
export function dueOccurrences(schedule: Schedule, since: number, now: number, catchUpMs: number): Due {
  const normalized = validateSchedule(schedule);
  assertFiniteTimestamp(since, 'since');
  assertFiniteTimestamp(now, 'now');
  if (typeof catchUpMs !== 'number' || !Number.isFinite(catchUpMs) || catchUpMs < 0) {
    throw new ScheduleError('catchUpMs must be a non-negative finite number.');
  }

  const next = nextOccurrences(normalized, now, 1)[0] ?? null;
  if (now <= since) {
    return { run: null, missed: [], missedTotal: 0, next };
  }

  if (normalized.kind === 'once') {
    const instant = onceInstant(normalized);
    if (instant > since && instant <= now) {
      return buildDue(1, [instant], now, catchUpMs, next);
    }
    return { run: null, missed: [], missedTotal: 0, next };
  }

  const collectLimit = MAX_MISSED + 1;

  if (normalized.kind === 'hourly') {
    const totalDue = countHourlyInRange(since, now, normalized.minute, normalized.timeZone);
    if (totalDue === 0) return { run: null, missed: [], missedTotal: 0, next };
    const latest = collectLatestHourly(since, now, normalized.minute, normalized.timeZone, Math.min(totalDue, collectLimit));
    return buildDue(totalDue, latest, now, catchUpMs, next);
  }

  const { days, timeMinutes } = calendarConfig(normalized);
  const firstDay = firstCalendarDayAfter(since, days, timeMinutes, normalized.timeZone);
  const lastDay = lastCalendarDayAtOrBefore(now, days, timeMinutes, normalized.timeZone);
  const totalDue = countAllowedDays(firstDay, lastDay, days);
  if (totalDue === 0) return { run: null, missed: [], missedTotal: 0, next };

  const latest: number[] = [];
  let day = lastDay;
  while (day >= firstDay && latest.length < collectLimit) {
    latest.push(calendarOccurrenceOnDay(day, timeMinutes, normalized.timeZone));
    day = prevAllowedDayOnOrBefore(day - 1, days);
  }
  latest.reverse();
  return buildDue(totalDue, latest, now, catchUpMs, next);
}
