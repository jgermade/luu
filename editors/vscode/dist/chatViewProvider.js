"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.LuuChatViewProvider = void 0;
const vscode = require("vscode");
class LuuChatViewProvider {
    _extensionUri;
    _client;
    static viewType = 'luu.chatView';
    _view;
    _lastHello;
    constructor(_extensionUri, _client) {
        this._extensionUri = _extensionUri;
        this._client = _client;
        this._client.on('message', (msg) => {
            if (msg.type === 'hello') {
                this._lastHello = msg;
            }
            this._view?.webview.postMessage(msg);
        });
        this._client.on('error', (err) => {
            this._view?.webview.postMessage({
                type: 'failed',
                turn: 0,
                message: `Client error: ${err.message}`,
            });
        });
        this._client.on('exit', (code, signal) => {
            this._view?.webview.postMessage({
                type: 'failed',
                turn: 0,
                message: `Luu subprocess exited with code ${code} (${signal})`,
            });
        });
    }
    resolveWebviewView(webviewView, _context, _token) {
        this._view = webviewView;
        webviewView.webview.options = {
            enableScripts: true,
            localResourceRoots: [this._extensionUri],
        };
        webviewView.webview.html = this._getHtmlForWebview(webviewView.webview);
        webviewView.webview.onDidReceiveMessage((data) => {
            switch (data.command) {
                case 'ready':
                    if (this._lastHello) {
                        webviewView.webview.postMessage(this._lastHello);
                    }
                    break;
                case 'prompt':
                    try {
                        this._client.sendPrompt(data.text);
                    }
                    catch (err) {
                        vscode.window.showErrorMessage(`Failed to send prompt: ${err.message}`);
                    }
                    break;
                case 'cancel':
                    this._client.cancel();
                    break;
                case 'approve_job':
                    try {
                        this._client.approveJob(data.job, data.amendment);
                    }
                    catch (err) {
                        vscode.window.showErrorMessage(`Failed to approve job: ${err.message}`);
                    }
                    break;
                case 'reject_job':
                    try {
                        this._client.rejectJob(data.job);
                    }
                    catch (err) {
                        vscode.window.showErrorMessage(`Failed to reject job: ${err.message}`);
                    }
                    break;
                case 'close_job':
                    try {
                        this._client.closeJob(data.job);
                    }
                    catch (err) {
                        vscode.window.showErrorMessage(`Failed to close job: ${err.message}`);
                    }
                    break;
            }
        });
    }
    _getHtmlForWebview(webview) {
        const scriptUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'chat.js'));
        const styleUri = webview.asWebviewUri(vscode.Uri.joinPath(this._extensionUri, 'media', 'chat.css'));
        const nonce = getNonce();
        return `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src ${webview.cspSource} 'unsafe-inline'; script-src 'nonce-${nonce}';">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <link href="${styleUri}" rel="stylesheet">
  <title>Luu Chat & Gate</title>
</head>
<body>
  <div id="header">
    <span id="backend-model-label">Connecting to luu...</span>
    <span id="status-indicator">Ready</span>
  </div>
  <div id="transcript"></div>
  <div id="composer">
    <textarea id="prompt-input" placeholder="Ask luu or enter a task... (Enter to send, Shift+Enter for newline)"></textarea>
    <div class="composer-controls">
      <button id="cancel-btn" class="danger" style="display: none;">Cancel Turn</button>
      <div style="flex: 1;"></div>
      <button id="send-btn" class="primary">Send</button>
    </div>
  </div>
  <script nonce="${nonce}" src="${scriptUri}"></script>
</body>
</html>`;
    }
}
exports.LuuChatViewProvider = LuuChatViewProvider;
function getNonce() {
    let text = '';
    const possible = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
    for (let i = 0; i < 32; i++) {
        text += possible.charAt(Math.floor(Math.random() * possible.length));
    }
    return text;
}
//# sourceMappingURL=chatViewProvider.js.map