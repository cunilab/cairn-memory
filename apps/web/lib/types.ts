export type ListedProject = {
  project_id: string;
  repository: string;
  name: string;
  default_branch?: string | null;
  created_at: string;
  updated_at: string;
  role: string;
  share_policy?: string | null;
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

export type PersonalMemory = {
  memory_id: string;
  content: string;
  kind?: string;
  created_at?: string;
  scope: string;
  /** Set on a project decision, never on a personal memory. */
  project_id?: string | null;
  context?: { branch?: string | null; repository?: string | null };
  lifecycle?: { sensitivity?: string };
};

export type ProjectMemory = {
  memory_id: string;
  content: string;
  kind?: string;
  scope: string;
  verified_commit?: string | null;
  supersedes?: string | null;
  lifecycle?: {
    freshness?: number;
    sensitivity?: string;
  };
  share_reason?: string | null;
  created_at?: string;
};

export type ProjectTeam = {
  project_id: string;
  name: string;
  created_at: string;
  members: Array<{
    person_id: string;
    email: string;
    created_at: string;
  }>;
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

export type ProjectRevocation = {
  memory_id: string;
  member_person_id: string;
  member_email?: string | null;
  created_at: string;
  revoked_at?: string | null;
};

export type ShareCandidate = {
  memory_id: string;
  reason: string;
  not_auto_shared_reason: string;
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
