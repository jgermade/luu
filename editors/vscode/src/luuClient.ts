import * as cp from 'child_process';
import * as readline from 'readline';
import { EventEmitter } from 'events';
import { ClientMessage, ServerMessage, JobId } from './types';

export interface LuuClientOptions {
  executablePath?: string;
  cwd: string;
  backend?: string;
  model?: string;
  onStderr?: (chunk: string) => void;
}

export class LuuClient extends EventEmitter {
  private child: cp.ChildProcessWithoutNullStreams | null = null;
  private rl: readline.Interface | null = null;
  private options: LuuClientOptions;
  private isDisposed = false;

  constructor(options: LuuClientOptions) {
    super();
    this.options = options;
  }

  public start(): void {
    if (this.child) {
      return;
    }
    this.isDisposed = false;

    const bin = this.options.executablePath && this.options.executablePath.trim() !== ''
      ? this.options.executablePath
      : 'luu';

    const args = ['stdio'];
    if (this.options.backend) {
      args.push('--backend', this.options.backend);
    }
    if (this.options.model) {
      args.push('--model', this.options.model);
    }

    try {
      this.child = cp.spawn(bin, args, {
        cwd: this.options.cwd,
        env: { ...process.env },
        stdio: ['pipe', 'pipe', 'pipe'],
      });
    } catch (err) {
      this.emit('error', err);
      return;
    }

    this.child.on('error', (err) => {
      this.emit('error', err);
    });

    this.child.on('exit', (code, signal) => {
      this.emit('exit', code, signal);
      this.cleanup();
    });

    if (this.child.stderr) {
      this.child.stderr.setEncoding('utf8');
      this.child.stderr.on('data', (chunk: string) => {
        if (this.options.onStderr) {
          this.options.onStderr(chunk);
        }
      });
    }

    if (this.child.stdout) {
      this.rl = readline.createInterface({
        input: this.child.stdout,
        crlfDelay: Infinity,
      });

      this.rl.on('line', (line: string) => {
        const trimmed = line.trim();
        if (!trimmed) {
          return;
        }
        try {
          const msg = JSON.parse(trimmed) as ServerMessage;
          this.emit('message', msg);
        } catch (err) {
          this.emit('parse_error', err, trimmed);
        }
      });
    }
  }

  public send(msg: ClientMessage): void {
    if (!this.child || !this.child.stdin || this.child.killed) {
      throw new Error('luu stdio process is not running');
    }
    const json = JSON.stringify(msg);
    this.child.stdin.write(`${json}\n`);
  }

  public sendPrompt(text: string): void {
    this.send({ type: 'prompt', text });
  }

  public cancel(): void {
    this.send({ type: 'cancel' });
  }

  public approveJob(
    job: JobId,
    amendment?: {
      files?: string[];
      writes?: string[];
      commands?: string[];
      closes_on?: string | null;
      network?: boolean | null;
      egress?: string[] | null;
    }
  ): void {
    this.send({
      type: 'approve_job',
      job,
      files: amendment?.files,
      writes: amendment?.writes,
      commands: amendment?.commands,
      closes_on: amendment?.closes_on,
      network: amendment?.network,
      egress: amendment?.egress,
    });
  }

  public rejectJob(job: JobId): void {
    this.send({ type: 'reject_job', job });
  }

  public closeJob(job: JobId): void {
    this.send({ type: 'close_job', job });
  }

  public reopenJob(job: JobId): void {
    this.send({ type: 'reopen_job', job });
  }

  public restart(): void {
    this.dispose();
    this.start();
  }

  public dispose(): void {
    this.isDisposed = true;
    this.cleanup();
  }

  private cleanup(): void {
    if (this.rl) {
      this.rl.close();
      this.rl = null;
    }
    if (this.child) {
      try {
        if (!this.child.killed) {
          this.child.kill('SIGTERM');
        }
      } catch {
        // ignore
      }
      this.child = null;
    }
  }

  public isRunning(): boolean {
    return this.child !== null && !this.child.killed;
  }
}
