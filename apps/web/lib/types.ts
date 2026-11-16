export type ListedProject = {
  project_id: string;
  repository: string;
  name: string;
  default_branch?: string | null;
  created_at: string;
  updated_at: string;
  role: string;
};

export type ProjectBranch = {
  branch_id: string;
  name: string;
  status: string;
  status_reason?: string | null;
  merge_commit?: string | null;
  head_commit?: string | null;
};

export type Account = {
  person_id: string;
  email: string;
  is_admin?: boolean;
};

export type AccountRow = Required<Account> & { created_at: string };

export type ProjectMemory = {
  memory_id: string;
  title: string;
  summary: string;
  details?: string | null;
  reason: string;
  applies_to: string[];
  evidence: string[];
  tags: string[];
  entities: string[];
  status: "active" | "stale" | "superseded";
  pinned: boolean;
  importance?: number | null;
  author?: string | null;
  decision_context?: string | null;
  alternatives: string[];
  consequences?: string | null;
  rule?: string | null;
  exceptions?: string | null;
  symptom?: string | null;
  cause?: string | null;
  fix?: string | null;
  steps: string[];
  when?: string | null;
  statement?: string | null;
  source?: string | null;
  preference?: string | null;
  example?: string | null;
  content: string;
  kind?: string;
  scope: "project" | "personal" | "global";
  verified_commit?: string | null;
  supersedes?: string | null;
  project_id?: string | null;
  context?: { branch?: string | null; repository?: string | null };
  lifecycle?: { freshness?: number; sensitivity?: string; importance?: number };
  published_by?: string | null;
  changed_by?: string | null;
  changed_at?: string | null;
  created_at?: string;
};

export type PersonalMemory = ProjectMemory;

export type ProjectMember = {
  person_id: string;
  email: string;
  role: "owner" | "member";
  /** Unix seconds as a string. */
  joined_at: string;
};

export type LifecycleSummary = {
  temporary_observations: number;
  durable_memories: number;
  sensitive_items: number;
  expired_items: number;
};

export type ProjectConflict = { memory_ids: string[] };

export type ProjectFreshnessEvent = {
  event_id: string;
  memory_id: string;
  previous_commit?: string | null;
  commit: string;
  action: string;
  reason?: string | null;
  created_at: string;
};

export type Device = {
  device_id: string;
  name: string;
  status: string;
  registered_at: string;
  last_seen_at: string;
};

export type DeviceRegistration = {
  device: Device;
  token: string;
};
