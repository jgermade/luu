const path = require('path');
const { LuuClient } = require('./dist/luuClient');

async function run() {
  const root = path.resolve(__dirname, '../..');
  const bin = path.join(root, 'target/debug/luu');

  console.log('Testing LuuClient with:', bin);

  const client = new LuuClient({
    executablePath: bin,
    cwd: root,
    onStderr: (chunk) => {
      // Diagnostic output
      // process.stderr.write(`[stderr] ${chunk}`);
    }
  });

  let helloReceived = false;
  let jobProposedReceived = false;
  let jobApprovedReceived = false;
  let turnStartedReceived = false;
  let turnEndedReceived = false;
  let proposedJobId = null;

  client.on('message', (msg) => {
    // console.log('Message:', msg.type);
    if (msg.type === 'hello') {
      helloReceived = true;
      console.log('  -> Received hello: protocol =', msg.protocol);
      // Send a prompt
      client.sendPrompt('hello from vscode extension test');
    } else if (msg.type === 'turn_started') {
      turnStartedReceived = true;
      console.log('  -> Received turn_started:', msg.turn);
    } else if (msg.type === 'job_proposed') {
      jobProposedReceived = true;
      proposedJobId = msg.job;
      console.log('  -> Received job_proposed for job #', proposedJobId);
      // Approve it!
      client.approveJob(proposedJobId, {
        files: msg.plan.files,
        writes: msg.plan.writes,
        network: false,
      });
    } else if (msg.type === 'job_approved') {
      jobApprovedReceived = true;
      console.log('  -> Received job_approved for job #', msg.job);
    } else if (msg.type === 'ended') {
      turnEndedReceived = true;
      console.log('  -> Received ended for turn #', msg.turn);
      client.dispose();
    }
  });

  client.on('error', (err) => {
    console.error('Client error:', err);
    process.exit(1);
  });

  client.start();

  await new Promise((resolve) => setTimeout(resolve, 3000));

  if (!helloReceived) {
    throw new Error('Did not receive hello');
  }
  if (!turnStartedReceived) {
    throw new Error('Did not receive turn_started');
  }
  console.log('LuuClient test passed successfully!');
  process.exit(0);
}

run().catch((err) => {
  console.error(err);
  process.exit(1);
});
