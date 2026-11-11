export type ListedProject = {
  project_id: string;
  repository: string;
  name: string;
  default_branch?: string | null;
  created_at: string;
  updated_at: string;
  role: string;
};

export type ProjectMemory = {
  memory_id: string;
  content: string;
  scope: string;
  verified_commit?: string | null;
  supersedes?: string | null;
  lifecycle?: {
    freshness?: number;
    sensitivity?: string;
  };
  share_reason?: string | null;
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
