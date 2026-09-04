// Protocol v5 types for luu stdio bridge

/// What this client speaks. Sent as the first message so a host that speaks
/// something else refuses it out loud, rather than by misreading the next one.
export const PROTOCOL = 5;
export const RECORD_FORMAT = 7;

export type TurnId = number;
export type JobId = number;

export type RefusalReason = 'busy' | 'pending' | 'job' | 'not_granted' | 'version' | 'signature';

export type PlanSource = 'model' | 'prose' | 'written';
export type ClosedBy = 'user' | 'exit_code';
export type ApprovedBy = { by: 'operator' } | { by: 'key'; name: string };

/// Ed25519 over the canonical rendering of the grant, made by `luu key sign`.
export interface Signature {
  by: string;
  sig: string;
}

export interface Plan {
  tasks?: string[];
  steps?: string[];
  files?: string[];
  writes?: string[];
  commands?: string[];
  closes_on?: string | null;
  network?: boolean | null;
  egress?: string[] | null;
}

export interface Verdict {
  allowed: boolean;
  missing?: string[];
  reason?: string;
}

export interface Usage {
  prompt_tokens: number;
  completion_tokens: number;
  total_tokens: number;
}

export type EndReason = 'complete' | 'length' | 'tool_use' | 'stop' | 'cancel' | string;

// Client -> Server messages (NDJSON)
export type ClientMessage =
  | { type: 'hello'; protocol: number; format?: number }
  | { type: 'prompt'; text: string }
  | { type: 'cancel' }
  | {
      type: 'approve_job';
      job: JobId;
      files?: string[];
      writes?: string[];
      commands?: string[];
      closes_on?: string | null;
      network?: boolean | null;
      egress?: string[] | null;
      signature?: Signature | null;
    }
  | { type: 'reject_job'; job: JobId }
  | { type: 'close_job'; job: JobId }
  | { type: 'reopen_job'; job: JobId };

// Server -> Client messages (NDJSON)
export type ServerMessage =
  | {
      type: 'hello';
      protocol: number;
      backend: string;
      model: string;
      turn: TurnId | null;
      /// What the host calls this session. An approval is signed against it.
      session?: string | null;
    }
  | {
      type: 'turn_started';
      turn: TurnId;
      prompt: string;
      job?: JobId | null;
    }
  | {
      type: 'job_proposed';
      job: JobId;
      objective: string;
      plan: Plan;
      source?: PlanSource | null;
    }
  | {
      type: 'job_approved';
      job: JobId;
      plan: Plan;
      /// Which authority approved it. Absent means the operator, which is what
      /// every approval before signatures existed was.
      approved_by?: ApprovedBy | null;
    }
  | {
      type: 'job_closed';
      job: JobId;
      summary: string;
      by?: ClosedBy | null;
    }
  | {
      type: 'job_reopened';
      job: JobId;
    }
  | {
      type: 'job_rejected';
      job: JobId;
    }
  | {
      type: 'token';
      turn: TurnId;
      text: string;
    }
  | {
      type: 'ended';
      turn: TurnId;
      reason: EndReason;
      usage?: Usage | null;
    }
  | {
      type: 'failed';
      turn: TurnId;
      message: string;
    }
  | {
      type: 'refused';
      request: string;
      reason: RefusalReason;
      detail: string;
    }
  | {
      type: 'evicted';
      turn: TurnId;
      turns: TurnId[];
      tokens: number;
      counter: string;
      policy: string;
    }
  | {
      type: 'tool_call';
      turn: TurnId;
      step: number;
      name: string;
      arguments: Record<string, unknown>;
    }
  | {
      type: 'tool_result';
      turn: TurnId;
      step: number;
      name: string;
      verdict: Verdict;
      error?: string | null;
      output: string;
      truncated: boolean;
      duration_ms: number;
      exit_code?: number | null;
      signal?: number | null;
      stdout?: string | null;
      stderr?: string | null;
    };
