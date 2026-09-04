(function () {
  const vscode = acquireVsCodeApi();

  const transcript = document.getElementById('transcript');
  const promptInput = document.getElementById('prompt-input');
  const sendBtn = document.getElementById('send-btn');
  const cancelBtn = document.getElementById('cancel-btn');
  const statusIndicator = document.getElementById('status-indicator');
  const backendModelLabel = document.getElementById('backend-model-label');

  let activeTurnId = null;
  let activeAssistantMsgEl = null;
  let activeToolCards = new Map();

  function escapeHtml(str) {
    if (!str) return '';
    return str
      .replace(/&/g, '&amp;')
      .replace(/</g, '&lt;')
      .replace(/>/g, '&gt;')
      .replace(/"/g, '&quot;')
      .replace(/'/g, '&#039;');
  }

  function appendUserMessage(text) {
    const el = document.createElement('div');
    el.className = 'msg user';
    el.innerHTML = `<div class="msg-header">You</div><div class="msg-content">${escapeHtml(text)}</div>`;
    transcript.appendChild(el);
    transcript.scrollTop = transcript.scrollHeight;
  }

  function getOrCreateAssistantMessage(turn) {
    if (activeAssistantMsgEl && activeTurnId === turn) {
      return activeAssistantMsgEl;
    }
    activeTurnId = turn;
    const el = document.createElement('div');
    el.className = 'msg assistant';
    el.innerHTML = `<div class="msg-header">Luu</div><div class="msg-content"></div>`;
    transcript.appendChild(el);
    activeAssistantMsgEl = el;
    transcript.scrollTop = transcript.scrollHeight;
    return el;
  }

  function renderJobGate(job, objective, plan, source) {
    const card = document.createElement('div');
    card.className = 'gate-card';
    card.id = `gate-job-${job}`;

    const tasks = plan.tasks || plan.steps || [];
    const files = plan.files || [];
    const writes = plan.writes || [];
    const network = plan.network ? 'Yes' : 'No';
    const closesOn = plan.closes_on ? `<code>${escapeHtml(plan.closes_on)}</code>` : 'None';

    const tasksHtml = tasks.length > 0
      ? `<ul class="gate-list">${tasks.map(t => `<li>${escapeHtml(t)}</li>`).join('')}</ul>`
      : '<div style="opacity:0.6;font-size:11px;">No tasks declared</div>';

    const filesHtml = files.length > 0 ? files.map(f => `<code>${escapeHtml(f)}</code>`).join(', ') : 'None';
    const writesHtml = writes.length > 0 ? writes.map(w => `<code>${escapeHtml(w)}</code>`).join(', ') : 'None';

    card.innerHTML = `
      <div class="gate-title">
        <span>Job #${job}: ${escapeHtml(objective)}</span>
        <span class="gate-source">${escapeHtml(source || 'model')}</span>
      </div>
      <div class="gate-section">
        <div class="gate-section-title">Checklist:</div>
        ${tasksHtml}
      </div>
      <div class="gate-section">
        <div class="gate-section-title">Reads:</div>
        <div>${filesHtml}</div>
      </div>
      <div class="gate-section">
        <div class="gate-section-title">Writes:</div>
        <div>${writesHtml}</div>
      </div>
      <div class="gate-section">
        <span class="gate-section-title">Network:</span> ${network} |
        <span class="gate-section-title">Closes on:</span> ${closesOn}
      </div>
      <div class="gate-actions" id="gate-actions-${job}">
        <button class="primary" id="approve-job-${job}">Approve Job</button>
        <button class="secondary" id="amend-job-${job}">Amend & Approve</button>
        <button class="danger" id="reject-job-${job}">Reject</button>
      </div>
    `;

    transcript.appendChild(card);
    transcript.scrollTop = transcript.scrollHeight;

    document.getElementById(`approve-job-${job}`).onclick = () => {
      vscode.postMessage({
        command: 'approve_job',
        job: job,
        amendment: {
          files: files,
          writes: writes,
          network: plan.network,
          closes_on: plan.closes_on,
        }
      });
      disableGateActions(job, 'Approving...');
    };

    document.getElementById(`amend-job-${job}`).onclick = () => {
      const extraWrites = prompt("Additional write paths (comma-separated):", "");
      let parsedWrites = [...writes];
      if (extraWrites) {
        extraWrites.split(',').forEach(p => {
          const trimmed = p.trim();
          if (trimmed && !parsedWrites.includes(trimmed)) parsedWrites.push(trimmed);
        });
      }
      vscode.postMessage({
        command: 'approve_job',
        job: job,
        amendment: {
          files: files,
          writes: parsedWrites,
          network: plan.network,
          closes_on: plan.closes_on,
        }
      });
      disableGateActions(job, 'Approving with amendments...');
    };

    document.getElementById(`reject-job-${job}`).onclick = () => {
      vscode.postMessage({ command: 'reject_job', job: job });
      disableGateActions(job, 'Rejected');
    };
  }

  function disableGateActions(job, statusText) {
    const el = document.getElementById(`gate-actions-${job}`);
    if (el) {
      el.innerHTML = `<span style="font-size:11px;opacity:0.8;font-style:italic;">${escapeHtml(statusText)}</span>`;
    }
  }

  // Handle messages from the extension host
  window.addEventListener('message', event => {
    const message = event.data;
    switch (message.type) {
      case 'hello':
        backendModelLabel.innerText = `${message.backend} / ${message.model} (proto v${message.protocol})`;
        break;

      case 'turn_started':
        statusIndicator.innerText = `Turn #${message.turn} running...`;
        cancelBtn.style.display = 'inline-block';
        sendBtn.disabled = true;
        appendUserMessage(message.prompt);
        getOrCreateAssistantMessage(message.turn);
        break;

      case 'token': {
        const asstEl = getOrCreateAssistantMessage(message.turn);
        const content = asstEl.querySelector('.msg-content');
        content.innerText += message.text;
        transcript.scrollTop = transcript.scrollHeight;
        break;
      }

      case 'tool_call': {
        const card = document.createElement('div');
        card.className = 'tool-card';
        const key = `${message.turn}-${message.step}`;
        card.innerHTML = `
          <div class="tool-header">
            <span class="tool-name">${escapeHtml(message.name)}</span>
            <span class="tool-verdict" style="background:#555;">running</span>
          </div>
          <div class="tool-body">${escapeHtml(JSON.stringify(message.arguments, null, 2))}</div>
        `;
        transcript.appendChild(card);
        transcript.scrollTop = transcript.scrollHeight;
        activeToolCards.set(key, card);
        break;
      }

      case 'tool_result': {
        const key = `${message.turn}-${message.step}`;
        const card = activeToolCards.get(key);
        if (card) {
          const verdictEl = card.querySelector('.tool-verdict');
          const isAllowed = message.verdict.allowed;
          verdictEl.className = `tool-verdict ${isAllowed ? 'verdict-allowed' : 'verdict-denied'}`;
          verdictEl.innerText = isAllowed ? `${message.duration_ms}ms` : 'denied';

          const bodyEl = card.querySelector('.tool-body');
          let outputDetails = message.output || '';
          if (message.exit_code !== undefined && message.exit_code !== null) {
            outputDetails = `Exit code: ${message.exit_code}\n${outputDetails}`;
          }
          if (message.error) {
            outputDetails += `\nError: ${message.error}`;
          }
          bodyEl.innerText += `\n\n--- Result ---\n${outputDetails}`;
        }
        break;
      }

      case 'job_proposed':
        renderJobGate(message.job, message.objective, message.plan, message.source);
        statusIndicator.innerText = `Job #${message.job} proposed. Awaiting approval.`;
        break;

      case 'job_approved':
        disableGateActions(message.job, 'Approved');
        statusIndicator.innerText = `Job #${message.job} approved.`;
        break;

      case 'job_rejected':
        disableGateActions(message.job, 'Rejected');
        statusIndicator.innerText = `Job #${message.job} rejected.`;
        break;

      case 'job_closed': {
        const banner = document.createElement('div');
        banner.className = 'evicted-banner';
        banner.innerHTML = `<strong>Job #${message.job} closed:</strong> ${escapeHtml(message.summary)}`;
        transcript.appendChild(banner);
        transcript.scrollTop = transcript.scrollHeight;
        break;
      }

      case 'refused': {
        const banner = document.createElement('div');
        banner.className = 'refused-banner';
        banner.innerHTML = `<strong>Refused (${escapeHtml(message.reason)}):</strong> ${escapeHtml(message.detail)}`;
        transcript.appendChild(banner);
        transcript.scrollTop = transcript.scrollHeight;
        break;
      }

      case 'evicted': {
        const banner = document.createElement('div');
        banner.className = 'evicted-banner';
        banner.innerHTML = `Evicted turns ${message.turns.join(', ')} (${message.tokens} tokens freed via ${message.policy})`;
        transcript.appendChild(banner);
        transcript.scrollTop = transcript.scrollHeight;
        break;
      }

      case 'ended':
        statusIndicator.innerText = `Ready (Turn #${message.turn} finished: ${message.reason})`;
        cancelBtn.style.display = 'none';
        sendBtn.disabled = false;
        activeAssistantMsgEl = null;
        break;

      case 'failed':
        statusIndicator.innerText = `Turn #${message.turn} failed: ${message.message}`;
        cancelBtn.style.display = 'none';
        sendBtn.disabled = false;
        activeAssistantMsgEl = null;
        break;
    }
  });

  function sendPrompt() {
    const text = promptInput.value.trim();
    if (!text) return;
    promptInput.value = '';
    vscode.postMessage({ command: 'prompt', text });
  }

  sendBtn.addEventListener('click', sendPrompt);

  promptInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' && !e.shiftKey) {
      e.preventDefault();
      sendPrompt();
    }
  });

  cancelBtn.addEventListener('click', () => {
    vscode.postMessage({ command: 'cancel' });
  });

  vscode.postMessage({ command: 'ready' });
})();
