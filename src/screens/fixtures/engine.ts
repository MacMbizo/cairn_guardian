// Sample engine data so the This week and Settings screens render in the browser preview.
import type { EngineStatus, Reminder, WeeklyNote } from "../../bindings/client";

const DAY = 86_400_000;
const now = Date.now();

export const fixtureReminders: Reminder[] = [
  {
    id: "r1",
    kind: "only_copy",
    subject: "backup",
    level: "convenient",
    title: "3.2 GB exist only on this PC",
    detail: "Your Backup drive Seagate B hasn't been plugged in for 9 days.",
    bytes: 3_435_973_837,
    action: { type: "open", screen: "drives" },
    state: "open",
    first_raised_at: now - 2 * DAY,
    last_shown_at: now - DAY,
  },
  {
    id: "r2",
    kind: "janitor",
    subject: "downloads",
    level: "note",
    title: "2.1 GB of old installers and repeat downloads",
    detail: "Ready to review. Nothing is removed until you approve, and quarantine keeps it 30 days.",
    bytes: 2_254_857_830,
    action: { type: "batch", batch_id: "b1" },
    state: "open",
    first_raised_at: now - DAY,
    last_shown_at: null,
  },
];

export const fixtureNote: WeeklyNote = {
  week: "2026-11-22",
  headline: "A quiet week. 2 things to look at when you have a minute.",
  needs_you: fixtureReminders,
  did: ["Indexed 1,284 new files", "Topped up the backup 3 times", "Catalogued 1 drive"],
  safety: "3.2 GB exist only on this PC. Backup drive last seen 9 days ago.",
  space: ["Local Disk (C:) fills up in about 11 weeks; Downloads is growing fastest."],
  tidy: "2.1 GB of old installers and repeat downloads, ready to review.",
  projects: "6 projects have copies in 2\u20133 places.",
  about_aegis: ["I stopped reminding you about quarantine about to empty; it's here instead."],
  read: false,
};

export const fixtureStatus: EngineStatus = {
  paused_until: null,
  tray: "waiting",
  autostart: true,
  note_day: 6,
  note_hour: 18,
  shortcut: "CommandOrControl+Shift+Space",
  shortcut_ok: true,
  demoted: ["quarantine_expiry"],
};
