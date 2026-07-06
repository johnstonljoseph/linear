# Linear Syntax

VS Code syntax highlighting for `.lr` files.

This is a lightweight TextMate grammar. It highlights the current surface
syntax lexically; it does not run the Linear parser and is not intended to be a
semantic language server.

## Local Development

Open this folder in VS Code and press `F5` to launch an extension development
host.

To package and install it into the current VS Code profile:

```sh
npx --yes @vscode/vsce package --no-dependencies --out linear-syntax-0.0.1.vsix
code --install-extension linear-syntax-0.0.1.vsix --force
```

Reload VS Code after installing the extension.
