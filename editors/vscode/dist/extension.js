"use strict";
Object.defineProperty(exports, "__esModule", { value: true });
exports.activate = activate;
exports.deactivate = deactivate;
const vscode = require("vscode");
const path = require("path");
const fs = require("fs");
const luuClient_1 = require("./luuClient");
const chatViewProvider_1 = require("./chatViewProvider");
let client = null;
let outputChannel = null;
function resolveExecutable(configuredPath, workspaceRoot) {
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
function activate(context) {
    outputChannel = vscode.window.createOutputChannel('Luu Diagnostics');
    context.subscriptions.push(outputChannel);
    const workspaceRoot = vscode.workspace.workspaceFolders && vscode.workspace.workspaceFolders.length > 0
        ? vscode.workspace.workspaceFolders[0].uri.fsPath
        : process.cwd();
    const config = vscode.workspace.getConfiguration('luu');
    const configuredBin = config.get('executablePath');
    const backend = config.get('backend');
    const model = config.get('model');
    const executablePath = resolveExecutable(configuredBin, workspaceRoot);
    client = new luuClient_1.LuuClient({
        executablePath,
        cwd: workspaceRoot,
        backend: backend || undefined,
        model: model || undefined,
        onStderr: (chunk) => {
            outputChannel?.append(chunk);
        },
    });
    client.start();
    const provider = new chatViewProvider_1.LuuChatViewProvider(context.extensionUri, client);
    context.subscriptions.push(vscode.window.registerWebviewViewProvider(chatViewProvider_1.LuuChatViewProvider.viewType, provider, {
        webviewOptions: { retainContextWhenHidden: true },
    }));
    context.subscriptions.push(vscode.commands.registerCommand('luu.focusChat', () => {
        vscode.commands.executeCommand('luu.chatView.focus');
    }));
    context.subscriptions.push(vscode.commands.registerCommand('luu.restartSession', () => {
        outputChannel?.appendLine('[Extension] Restarting luu session...');
        client?.restart();
        vscode.window.showInformationMessage('Luu session restarted.');
    }));
    outputChannel.appendLine(`[Extension] Activated. luu executable: ${executablePath}`);
}
function deactivate() {
    if (client) {
        client.dispose();
        client = null;
    }
    if (outputChannel) {
        outputChannel.dispose();
        outputChannel = null;
    }
}
//# sourceMappingURL=extension.js.map