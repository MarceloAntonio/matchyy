# AGENTS.md

Instruções para assistentes de IA (Claude Code, Codex e afins) que trabalham neste repositório.

## O projeto

`matchyy` é um CLI em Rust que mostra a música tocando: capa (protocolo gráfico do kitty), visualizador de áudio (FFT do monitor via `parec`) e barra de progresso. O README explica a arquitetura.

## Regras

- **Um arquivo só.** Todo o código fica em `src/main.rs`. Divida em módulos apenas se o arquivo passar de ~1000 linhas, e nunca por antecipação.
- **Sem dependências novas** sem justificativa forte. As atuais são `crossterm`, `image` (só jpeg e png) e `rustfft`. Para o que dá para resolver com poucas linhas (base64, JSON, parsing de argumentos), escreva à mão.
- **Use ferramentas externas, não reimplemente:** `playerctl` para MPRIS, `parec` para o áudio, `curl` para baixar capas.
- **Opções novas vêm desligadas.** Sem a flag, o comportamento padrão não pode mudar. Toda opção nova entra em `Opts`, em `parse_args`, na constante `HELP`, na tabela do README e no teste `opcoes`.
- **Nada de piscar.** Não limpe a tela a cada quadro: escreva tudo no buffer `f` e reescreva cada linha no lugar com `\x1b[K`. A imagem do kitty só é reenviada quando a capa muda ou a janela muda de tamanho.
- **Português** em textos para o usuário, comentários e documentação, com acentuação correta. Identificadores do código ficam em inglês.

## Antes de entregar

```sh
cargo fmt --check
cargo clippy -- -D warnings
cargo test
cargo build --release
```

Mudanças visuais precisam ser vistas de verdade: rode o binário num kitty com música tocando e confira o resultado. Teste passando não basta para provar que o desenho está certo.

## Release

O binário da release precisa ser **estático (musl)**. Compilado no Arch, ele exige um glibc recente demais para outras distros. Para publicar:

```sh
# 1. aumente `version` no Cargo.toml e faça o commit
podman run --rm -v "$PWD":/src:Z -w /src docker.io/library/rust:alpine \
  sh -c 'apk add -q --no-cache musl-dev && cargo build --release --locked --target-dir /src/target/alpine'
mkdir -p dist && cp target/alpine/release/matchyy dist/matchyy-x86_64-linux
(cd dist && sha256sum matchyy-x86_64-linux > SHA256SUMS)
gh release create vX.Y.Z dist/matchyy-x86_64-linux dist/SHA256SUMS --title vX.Y.Z --notes "..."
```

O `install.sh` depende desses nomes exatos (`matchyy-x86_64-linux`, `SHA256SUMS`) e de `matchyy --version` imprimir `matchyy X.Y.Z`. Não os altere sem atualizar o script.

## Regressões

Todo bug corrigido ganha uma linha aqui e, quando der, um teste em `mod tests`.

| Bug | Causa | Teste |
|---|---|---|
| `install.sh` morria sem terminal (`curl | sh` em CI) em vez de dizer "Nada foi alterado" | `: </dev/tty` falha num builtin especial e o sh POSIX encerra | `setsid sh install.sh </dev/null` com uma versão velha instalada deve sair 0 |
