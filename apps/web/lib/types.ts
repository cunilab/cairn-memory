export type ListedProject = {
  project_id: string;
  repository: string;
  name: string;
  default_branch?: string | null;
  owner_id: string;
  created_at: number;
  updated_at: number;
  role: string;
};

/** Derived from the branch each project memory was saved on; there is no branch record. */
export type ProjectBranch = {
  branch: string;
  memories: number;
  last_updated: number;
};

export type Account = {
  person_id: string;
  email: string;
  is_admin?: boolean;
};

export type AccountRow = Required<Account> & { created_at: number; status: string };

export type QuarantinedJob = {
  job_id: string;
  kind: string;
  key: string;
  status: string;
  attempts: number;
  last_error: string | null;
  created_at: number;
  updated_at: number;
};

export type JobQueue = {
  pending: number;
  running: number;
  quarantined: number;
  quarantine: QuarantinedJob[];
};

export type MemoryLink = {
  relation: string;
  source: string;
  target: string;
  auto: boolean;
};

/** A memory as the server returns it. `fields` holds the keys of the memory's kind. */
export type ProjectMemory = {
  id: string;
  owner: string;
  project_id?: string | null;
  scope: "project" | "personal" | "global";
  kind: string;
  title: string;
  summary: string;
  reason?: string;
  fields?: Record<string, string | string[]>;
  evidence?: string[];
  evidence_count: number;
  context: { repo?: string | null; branch?: string | null; worktree?: string | null; commit?: string | null };
  entities?: { kind: string; name: string }[];
  links?: MemoryLink[];
  status: "active" | "stale" | "superseded" | "forgotten";
  expires_at?: number | null;
  created_at: number;
  updated_at: number;
};

export type PersonalMemory = ProjectMemory;

export type MemoryGraphNode = {
  memory_id: string;
  title: string;
  scope: "project" | "personal" | "global";
  project_id?: string | null;
  kind: string;
};

export type MemoryGraphEdge = {
  source_id: string;
  target_id: string;
  relation: string;
};

export type MemoryGraph = {
  nodes: MemoryGraphNode[];
  edges: MemoryGraphEdge[];
  truncated: boolean;
  node_cap: number;
};

export type ProjectMember = {
  person_id: string;
  email: string;
  role: "owner" | "member";
  joined_at: number;
};

/** Counts of the memories the account can see, and the background job queue. */
export type Diagnostics = {
  memories: number;
  by_kind: Record<string, number>;
  by_status: Record<string, number>;
  by_scope: Record<string, number>;
  jobs_pending: number;
  jobs_running: number;
  jobs_quarantined: number;
};

/** Two live project memories joined by a `contradicts` link. */
export type ProjectConflict = { memory_id: string; contradicts_id: string };

export type Device = {
  device_id: string;
  name: string;
  status: string;
  created_at: number;
  last_seen_at: number;
};

export type DeviceRegistration = {
  device: Device;
  token: string;
};
