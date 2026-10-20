"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.LuuClient = void 0;
const cp = require("child_process");
const readline = require("readline");
const events_1 = require("events");
const types_1 = require("./types");
class LuuClient extends events_1.EventEmitter {
    child = null;
    rl = null;
    options;
    isDisposed = false;
    constructor(options) {
        super();
        this.options = options;
    }
    start() {
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
        }
        catch (err) {
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
            this.child.stderr.on('data', (chunk) => {
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
            this.greet();
            this.rl.on('line', (line) => {
                const trimmed = line.trim();
                if (!trimmed) {
                    return;
                }
                try {
                    const msg = JSON.parse(trimmed);
                    this.emit('message', msg);
                }
                catch (err) {
                    this.emit('parse_error', err, trimmed);
                }
            });
        }
    }
    /// What this client speaks, before anything else it might say. Optional over
    /// stdio — the peer is the process this one spawned — and sent anyway, so a
    /// version mismatch is answered here rather than discovered as an
    /// unparseable frame three messages later.
    greet() {
        this.send({ type: 'hello', protocol: types_1.PROTOCOL, format: types_1.RECORD_FORMAT });
    }
    send(msg) {
        if (!this.child || !this.child.stdin || this.child.killed) {
            throw new Error('luu stdio process is not running');
        }
        const json = JSON.stringify(msg);
        this.child.stdin.write(`${json}\n`);
    }
    sendPrompt(text) {
        this.send({ type: 'prompt', text });
    }
    cancel() {
        this.send({ type: 'cancel' });
    }
    /// Approves the plan on the table, which closes the open draft and opens the
    /// job that plan describes — approving *is* closing.
    ///
    /// It names no job: a proposal is offered inside the draft and an id is what
    /// approval hands out, so there is nothing to name until the server answers.
    approvePlan(amendment) {
        this.send({
            type: 'approve_plan',
            files: amendment?.files,
            writes: amendment?.writes,
            commands: amendment?.commands,
            closes_on: amendment?.closes_on,
            network: amendment?.network,
            egress: amendment?.egress,
        });
    }
    /// Turns it down, or asks for more changes — the same answer. Nothing closes
    /// and nothing folds: the draft it was offered inside is still open.
    declinePlan() {
        this.send({ type: 'decline_plan' });
    }
    /// Asks for a plan over the draft as it stands. The plan is what the draft
    /// compacts to, so this is a planning call over a window somebody has
    /// already filled rather than over a prompt nobody has looked at.
    requestPlan() {
        this.send({ type: 'request_plan' });
    }
    closeJob(job) {
        this.send({ type: 'close_job', job });
    }
    reopenJob(job) {
        this.send({ type: 'reopen_job', job });
    }
    restart() {
        this.dispose();
        this.start();
    }
    dispose() {
        this.isDisposed = true;
        this.cleanup();
    }
    cleanup() {
        if (this.rl) {
            this.rl.close();
            this.rl = null;
        }
        if (this.child) {
            try {
                if (!this.child.killed) {
                    this.child.kill('SIGTERM');
                }
            }
            catch {
                // ignore
            }
            this.child = null;
        }
    }
    isRunning() {
        return this.child !== null && !this.child.killed;
    }
}
exports.LuuClient = LuuClient;
//# sourceMappingURL=luuClient.js.map