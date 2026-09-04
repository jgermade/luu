import * as vscode from 'vscode';
import { LuuClient } from './luuClient';
import { ServerMessage } from './types';

export class LuuChatViewProvider implements vscode.WebviewViewProvider {
  public static readonly viewType = 'luu.chatView';

  private _view?: vscode.WebviewView;
  private _lastHello?: ServerMessage;

  constructor(
    private readonly _extensionUri: vscode.Uri,
    private readonly _client: LuuClient
  ) {
    this._client.on('message', (msg: ServerMessage) => {
      if (msg.type === 'hello') {
        this._lastHello = msg;
      }
      this._view?.webview.postMessage(msg);
    });

    this._client.on('error', (err: Error) => {
      this._view?.webview.postMessage({
        type: 'failed',
        turn: 0,
        message: `Client error: ${err.message}`,
      });
    });

    this._client.on('exit', (code: number | null, signal: string | null) => {
      this._view?.webview.postMessage({
        type: 'failed',
        turn: 0,
        message: `Luu subprocess exited with code ${code} (${signal})`,
      });
    });
  }

  public resolveWebviewView(
    webviewView: vscode.WebviewView,
    _context: vscode.WebviewViewResolveContext,
    _token: vscode.CancellationToken
  ): void {
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
          } catch (err: any) {
            vscode.window.showErrorMessage(`Failed to send prompt: ${err.message}`);
          }
          break;
        case 'cancel':
          this._client.cancel();
          break;
        case 'approve_job':
          try {
            this._client.approveJob(data.job, data.amendment);
          } catch (err: any) {
            vscode.window.showErrorMessage(`Failed to approve job: ${err.message}`);
          }
          break;
        case 'reject_job':
          try {
            this._client.rejectJob(data.job);
          } catch (err: any) {
            vscode.window.showErrorMessage(`Failed to reject job: ${err.message}`);
          }
          break;
        case 'close_job':
          try {
            this._client.closeJob(data.job);
          } catch (err: any) {
            vscode.window.showErrorMessage(`Failed to close job: ${err.message}`);
          }
          break;
      }
    });
  }

  private _getHtmlForWebview(webview: vscode.Webview): string {
    const scriptUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, 'media', 'chat.js')
    );
    const styleUri = webview.asWebviewUri(
      vscode.Uri.joinPath(this._extensionUri, 'media', 'chat.css')
    );

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

function getNonce(): string {
  let text = '';
  const possible = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789';
  for (let i = 0; i < 32; i++) {
    text += possible.charAt(Math.floor(Math.random() * possible.length));
  }
  return text;
}
