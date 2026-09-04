import * as vscode from 'vscode';
import * as path from 'path';
import * as fs from 'fs';
import { LuuClient } from './luuClient';
import { LuuChatViewProvider } from './chatViewProvider';

let client: LuuClient | null = null;
let outputChannel: vscode.OutputChannel | null = null;

function resolveExecutable(configuredPath: string | undefined, workspaceRoot: string): string {
  if (configuredPath && configuredPath.trim()) {
    return configuredPath.trim();
  }

  // Check target/debug/luu or target/release/luu in workspace
  const debugBin = path.join(workspaceRoot, 'target', 'debug', 'luu');
  if (fs.existsSync(debugBin)) {
    return debugBin;
  }

  const releaseBin = path.join(workspaceRoot, 'target', 'release', 'luu');
  if (fs.existsSync(releaseBin)) {
    return releaseBin;
  }

  return 'luu';
}

export function activate(context: vscode.ExtensionContext): void {
  outputChannel = vscode.window.createOutputChannel('Luu Diagnostics');
  context.subscriptions.push(outputChannel);

  const workspaceRoot = vscode.workspace.workspaceFolders && vscode.workspace.workspaceFolders.length > 0
    ? vscode.workspace.workspaceFolders[0].uri.fsPath
    : process.cwd();

  const config = vscode.workspace.getConfiguration('luu');
  const configuredBin = config.get<string>('executablePath');
  const backend = config.get<string>('backend');
  const model = config.get<string>('model');

  const executablePath = resolveExecutable(configuredBin, workspaceRoot);

  client = new LuuClient({
    executablePath,
    cwd: workspaceRoot,
    backend: backend || undefined,
    model: model || undefined,
    onStderr: (chunk: string) => {
      outputChannel?.append(chunk);
    },
  });

  client.start();

  const provider = new LuuChatViewProvider(context.extensionUri, client);

  context.subscriptions.push(
    vscode.window.registerWebviewViewProvider(LuuChatViewProvider.viewType, provider, {
      webviewOptions: { retainContextWhenHidden: true },
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('luu.focusChat', () => {
      vscode.commands.executeCommand('luu.chatView.focus');
    })
  );

  context.subscriptions.push(
    vscode.commands.registerCommand('luu.restartSession', () => {
      outputChannel?.appendLine('[Extension] Restarting luu session...');
      client?.restart();
      vscode.window.showInformationMessage('Luu session restarted.');
    })
  );

  outputChannel.appendLine(`[Extension] Activated. luu executable: ${executablePath}`);
}

export function deactivate(): void {
  if (client) {
    client.dispose();
    client = null;
  }
  if (outputChannel) {
    outputChannel.dispose();
    outputChannel = null;
  }
}
