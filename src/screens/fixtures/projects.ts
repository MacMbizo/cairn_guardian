// Sample projects so the Projects screen renders in the browser preview (shaped like the owner's
// real dry run, 2026-11-22).
import type { Collection, Location, ProjectKind, Relationship } from "../../bindings/client";

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
  return { id, kind: "project", name, project_kind: kind, confidence, state, evidence, locations: locations(id), sub_projects: [] };
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
