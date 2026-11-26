#!/bin/sh
# Instalador do matchyy: baixa a última release do GitHub, confere o SHA-256 e
# coloca o binário no PATH. Se já estiver instalado, só atualiza depois de perguntar.
#
#   curl -fsSL https://raw.githubusercontent.com/MarceloAntonio/matchyy/main/install.sh | sh
#
# Variável opcional: MATCHYY_DIR=/outro/dir (padrão: ~/.local/bin)
set -eu

REPO="MarceloAntonio/matchyy"
ASSET="matchyy-x86_64-linux"

info() { printf '\033[1;36m::\033[0m %s\n' "$*"; }
warn() { printf '\033[1;33m!!\033[0m %s\n' "$*" >&2; }
die() {
    printf '\033[1;31mxx\033[0m %s\n' "$*" >&2
    exit 1
}

# curl só por HTTPS, inclusive nos redirecionamentos.
get() { curl -fsSL --proto '=https' --tlsv1.2 --retry 2 "$@"; }

# Pergunta s/N direto no terminal, já que no "curl | sh" a entrada padrão é o script.
ask() {
    # Subshell: no sh POSIX, erro de redirecionamento em builtin especial (`:`) mata o script.
    (true </dev/tty) 2>/dev/null || return 1
    printf '%s [s/N] ' "$1" >/dev/tty
    read -r ans </dev/tty || return 1
    case "$ans" in s | S | sim | Sim | y | Y | yes) return 0 ;; *) return 1 ;; esac
}

path_hint() {
    case ":$PATH:" in
    *":$1:"*) ;;
    *) warn "$1 não está no seu PATH. Adicione ao ~/.bashrc ou ~/.zshrc: export PATH=\"$1:\$PATH\"" ;;
    esac
}

deps() {
    for c in playerctl parec; do
        command -v "$c" >/dev/null 2>&1 || warn "o matchyy precisa do '$c' (instale pelo gerenciador de pacotes da sua distro)"
    done
}

main() {
    [ "$(uname -s)" = Linux ] || die "o matchyy só roda no Linux"
    [ "$(uname -m)" = x86_64 ] || die "ainda não há binário para $(uname -m); compile com: cargo install --git https://github.com/$REPO"
    for c in curl sha256sum install mktemp awk sort; do
        command -v "$c" >/dev/null 2>&1 || die "falta o comando '$c'"
    done

    info "Procurando a última versão..."
    url=$(get -I -o /dev/null -w '%{url_effective}' "https://github.com/$REPO/releases/latest") ||
        die "não consegui falar com o GitHub"
    tag=${url##*/}
    case "$tag" in v[0-9]*) ;; *) die "não encontrei nenhuma release em github.com/$REPO" ;; esac
    latest=${tag#v}

    # Já instalado? Atualiza no mesmo lugar, se der para escrever lá.
    dir=${MATCHYY_DIR:-$HOME/.local/bin}
    old=""
    if [ -x "$dir/matchyy" ]; then
        old="$dir/matchyy"
    elif old=$(command -v matchyy 2>/dev/null); then
        [ -z "${MATCHYY_DIR:-}" ] && [ -w "$(dirname "$old")" ] && dir=$(dirname "$old")
    fi
    if [ -n "$old" ]; then
        current=$("$old" --version 2>/dev/null | awk '{print $2}') || current=""
        if [ "$current" = "$latest" ]; then
            info "matchyy $current já está instalado em $old e é a última versão."
            path_hint "$(dirname "$old")"
            deps
            return 0
        fi
        if [ -n "$current" ] && [ "$(printf '%s\n%s\n' "$current" "$latest" | sort -V | tail -n1)" = "$current" ]; then
            info "matchyy $current em $old é mais novo que a última release ($latest). Nada a fazer."
            return 0
        fi
        info "matchyy ${current:-(versão desconhecida)} está instalado em $old. A última versão é $latest."
        if ! ask "Atualizar para $latest?"; then
            info "Nada foi alterado."
            return 0
        fi
    fi

    tmp=$(mktemp -d)
    trap 'rm -rf "$tmp"' EXIT INT TERM
    base="https://github.com/$REPO/releases/download/$tag"

    info "Baixando matchyy $latest..."
    get -o "$tmp/$ASSET" "$base/$ASSET" || die "falha ao baixar $ASSET"
    get -o "$tmp/SHA256SUMS" "$base/SHA256SUMS" || die "falha ao baixar SHA256SUMS"

    info "Conferindo o SHA-256..."
    expected=$(awk -v f="$ASSET" '$2 == f || $2 == "*" f { print $1 }' "$tmp/SHA256SUMS")
    [ -n "$expected" ] || die "o SHA256SUMS não traz o hash de $ASSET"
    actual=$(sha256sum "$tmp/$ASSET" | awk '{print $1}')
    [ "$actual" = "$expected" ] || die "o hash não confere (esperado $expected, recebido $actual). Instalação cancelada."

    chmod 755 "$tmp/$ASSET"
    [ "$("$tmp/$ASSET" --version 2>/dev/null)" = "matchyy $latest" ] || die "o binário baixado não rodou nesta máquina"

    # Troca atômica: copia ao lado e renomeia, para nunca deixar um binário pela metade.
    mkdir -p "$dir"
    install -m 755 "$tmp/$ASSET" "$dir/.matchyy.new"
    mv -f "$dir/.matchyy.new" "$dir/matchyy"
    info "matchyy $latest instalado em $dir/matchyy"
    path_hint "$dir"
    deps
}

main "$@"
