// Sample projects so the Projects screen renders in the browser preview (shaped like the owner's
// real dry run, 2026-11-22).
import type { Collection, Location, PlanStep, ProjectKind, ProjectPlan, Relationship } from "../../bindings/client";

const DAY = 86_400_000;
const NOW = Date.UTC(2026, 10, 22);

let n = 0;
function loc(cid: string, root: string, relationship: Relationship, files: number, shared: number, online = true): Location {
  n += 1;
  return {
    id: `l${n}`,
    collection_id: cid,
    volume_id: root.startsWith("D:") ? "pp-d" : "win-c",
    root_path: root,
    relationship,
    files,
    bytes: files * 40_000,
    newest_mtime: NOW - n * DAY,
    shared_files: shared,
    compared_by: "content",
    is_repo: false,
    online,
    fingerprint: `fp${n}`,
    keep_here: false,
  };
}

function project(
  id: string,
  name: string,
  kind: ProjectKind,
  confidence: number,
  evidence: string[],
  locations: (cid: string) => Location[],
  state = "candidate",
): Collection {
  return { id, kind: "project", name, project_kind: kind, confidence, state, evidence, locations: locations(id), sub_projects: [], plan_state: null };
}

const V2 = String.raw`C:\Users\Mac\Documents\Projectsv2`;

export const fixtureProjects: Collection[] = [
  project(
    "c-fx",
    "forexTrader",
    "software",
    0.85,
    [
      "project documents: architecture, PRD, prompt, roadmap, TASKS",
      "source code (80 files)",
      "brand and design files: favicon, logo",
      "documents, images and code together",
      String.raw`also at C:\Prj\forexTrader: same name and 302 files in common`,
    ],
    (c) => [
      loc(c, String.raw`${V2}\Dev Set\forexTrader`, "home", 396, 333),
      loc(c, String.raw`C:\Prj\forexTrader`, "older_subset", 356, 302),
      loc(c, String.raw`${V2}\F1`, "diverged", 43, 38),
      loc(c, String.raw`D:\Projectsv2\Dev Set\forexTrader`, "older_subset", 356, 302, false),
    ],
  ),
  project(
    "c-ds",
    "DrivingSchool",
    "software",
    0.98,
    ["is a code project (it has a build file)", "declares itself in package.json", "has a README"],
    (c) => [
      loc(c, String.raw`C:\Prj\DrivingSchool`, "home", 108, 79),
      loc(c, String.raw`${V2}\Dev Set\DrivingSchool`, "diverged", 108, 71),
    ],
  ),
  project(
    "c-inv",
    "Inv",
    "software",
    0.94,
    ["is a git repository", "has a README", "project documents: architecture, PRD, roadmap, spec"],
    (c) => [
      loc(c, String.raw`${V2}\Inv`, "home", 181, 98),
      loc(c, String.raw`${V2}\Inv2`, "version", 210, 25),
      loc(c, String.raw`${V2}\Inv3`, "version", 33, 26),
    ],
  ),
  project(
    "c-pp",
    "paperPilot",
    "software",
    0.98,
    ["is a git repository", "declares itself in package.json", "has a README"],
    (c) => [
      loc(c, String.raw`C:\Prj\paperPilot`, "home", 612, 505),
      loc(c, String.raw`${V2}\Dev Set\paperPilot`, "older_subset", 346, 279),
      loc(c, String.raw`${V2}\Pp1`, "different_material", 319, 3),
    ],
  ),
  project(
    "c-well",
    "wellness",
    "writing",
    0.65,
    ["project documents: persona, roadmap", "sits among other projects in Prj"],
    (c) => [
      loc(c, String.raw`C:\Prj\wellness`, "home", 191, 120),
      loc(c, String.raw`${V2}\Dev Set\wellness_app`, "unknown", 67, 0),
    ],
  ),
  project("c-chat", "chatbot", "software", 0.9, ["project documents: PRD, prompt", "documents, images and code together"], (c) => [
    loc(c, String.raw`${V2}\Dev Set\chatbot`, "home", 24, 20),
  ]),
  project("c-am", "AgentsM", "writing", 0.6, ["sits among other projects in Dev Set"], (c) => [
    loc(c, String.raw`${V2}\Dev Set\AgentsM`, "home", 8, 8),
  ]),
  project("c-aves", "aves", "writing", 0.6, ["sits among other projects in DevSet"], (c) => [
    loc(c, String.raw`C:\Prj\DevSet\aves`, "home", 68, 60),
  ]),
  project("c-trial", "trial", "other", 0.6, ["sits among other projects in Prj"], (c) => [
    loc(c, String.raw`C:\Prj\trial`, "home", 8, 8),
  ]),
  project(
    "c-mm",
    "mergemind",
    "software",
    0.98,
    ["is a git repository", "declares itself in requirements.txt, pyproject.toml", "has a README"],
    (c) => [loc(c, String.raw`C:\Prj\mergemind`, "home", 14_959, 9_000)],
  ),
  project(
    "c-ddu",
    "DDU v18.1.5.7",
    "other",
    0.92,
    ["has a README", "project documents: README"],
    (c) => [loc(c, String.raw`C:\Users\Mac\Downloads\DDU v18.1.5.7`, "home", 41, 30)],
    "left_as_is",
  ),
];

const PRJ = String.raw`C:\Prj`;
const leafOf = (p: string) => p.split("\\").pop() ?? p;
const parentLeaf = (p: string) => p.split("\\").slice(-2, -1)[0] ?? "";
const day = (ms: number) => new Date(ms).toISOString().slice(0, 10);

/** Plans for the sample projects, made by the same rules as the real planner (simplified). */
export function fixturePlans(list: Collection[]): ProjectPlan[] {
  const out: ProjectPlan[] = [];
  for (const c of list) {
    if (c.state === "left_as_is") continue;
    const home = c.locations.find((l) => l.relationship === "home") ?? c.locations[0];
    const onPc = c.locations.filter((l) => l.volume_id === home.volume_id);
    const away = c.locations.filter((l) => l.volume_id !== home.volume_id);
    const steps: PlanStep[] = [];
    let homeAfter = home.root_path;
    let target = `${PRJ}\\${leafOf(home.root_path)}`;
    const atTarget = onPc.find((l) => l !== home && l.root_path.toLowerCase() === target.toLowerCase());
    // A copy kept where it is keeps its name too: the main copy takes another.
    const occupant = atTarget?.keep_here ? undefined : atTarget;
    if (atTarget?.keep_here) target = `${target} (app ${new Date(home.newest_mtime).getFullYear()})`;
    if (home.root_path.startsWith(V2)) {
      if (occupant) {
        steps.push({ step: 0, kind: "move_aside", location_id: occupant.id, from: occupant.root_path, to: `${target} (old copy)`, fallback: null, reason: "Moved aside first.", warnings: [] });
      }
      steps.push({ step: 0, kind: "move_home", location_id: home.id, from: home.root_path, to: target, fallback: null, reason: `Moves whole to ${target} (your projects home).`, warnings: [] });
      homeAfter = target;
    }
    const versions = `${homeAfter}\\Versions`;
    for (const l of onPc.filter((x) => x !== home && !x.keep_here)) {
      const from = l === occupant ? `${target} (old copy)` : l.root_path;
      const dated = `${versions}\\${day(l.newest_mtime)} ${parentLeaf(l.root_path)}`;
      const step = (kind: PlanStep["kind"], to: string, reason: string, fallback: string | null = null): PlanStep => ({
        step: 0,
        kind,
        location_id: l.id,
        from,
        to,
        fallback,
        reason,
        warnings: [],
      });
      if (l.relationship === "older_subset" || l.relationship === "identical") steps.push(step("quarantine", from, "Older copy: everything here is in the main copy. To quarantine for 30 days.", dated));
      else if (l.relationship === "diverged") steps.push(step("to_versions", dated, `Changed on both sides: kept whole as a version in ${dated}.`));
      else if (l.relationship === "version") steps.push(step("to_versions", `${versions}\\${day(l.newest_mtime)} ${leafOf(l.root_path)}`, "An earlier version."));
      else if (l.relationship === "different_material") steps.push(step("to_material", `${homeAfter}\\Docs`, "Different material: brought into the main copy."));
    }
    steps.forEach((s, i) => (s.step = i + 1));
    const n = (k: PlanStep["kind"]) => steps.filter((s) => s.kind === k).length;
    const parts = [
      n("move_home") ? `Moves to ${homeAfter}` : "",
      n("quarantine") ? `${n("quarantine")} to quarantine` : "",
      n("to_versions") ? `${n("to_versions")} kept as versions` : "",
      n("to_material") ? `${n("to_material")} folder of material brought in` : "",
    ].filter(Boolean);
    const rank = steps.some((s) => s.kind === "to_versions" || s.kind === "to_material") ? 3 : n("quarantine") ? 1 : steps.length ? 2 : 9;
    out.push({
      collection_id: c.id,
      volume_id: home.volume_id,
      home_after: homeAfter,
      steps,
      waits_for: null,
      notes: [],
      rank,
      summary: parts.length ? `${parts.join(", ")}.` : "Nothing to do: it is already where it belongs.",
    });
    if (away.length) {
      out.push({
        collection_id: c.id,
        volume_id: away[0].volume_id,
        home_after: away[0].root_path,
        steps: [],
        waits_for: away[0].online ? null : "Passport D is unplugged.",
        notes: [],
        rank: 9,
        summary: "Copies on this drive are listed. Plans for other drives come later.",
      });
    }
  }
  return out;
}
