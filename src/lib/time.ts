const MINUTE = 60_000;

export function span(minutes: number): string {
  if (minutes < 60) return `${minutes} min`;
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return m ? `${h} h ${m} min` : `${h} h`;
}

/** A short age for lists: "12 min", "3 h", "6 d". */
export function age(ms: number): string {
  const minutes = Math.max(0, Math.floor(ms / MINUTE));
  if (minutes < 60) return `${minutes} min`;
  if (minutes < 24 * 60) return `${Math.floor(minutes / 60)} h`;
  return `${Math.floor(minutes / (24 * 60))} d`;
}

export function relative(due: number, now: number): string {
  const diff = due - now;
  if (diff <= 0) {
    const late = Math.floor(-diff / MINUTE);
    return late < 1 ? "due now" : `overdue ${span(late)}`;
  }
  return `in ${span(Math.ceil(diff / MINUTE))}`;
}

export function clock(ms: number): string {
  return new Date(ms).toLocaleTimeString("nb-NO", { hour: "2-digit", minute: "2-digit" });
}

export function isTomorrow(ms: number, now: number): boolean {
  return new Date(ms).toDateString() !== new Date(now).toDateString();
}
