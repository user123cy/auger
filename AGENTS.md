# Regras Obrigatórias para Qualquer IA Trabalhando no Auger

## 1. Commits e Git
- **NUNCA** adicione `Co-Authored-By: Claude <noreply@anthropic.com>` ou qualquer atribuição de IA
- Commits **secos, diretos**, como se fosse o próprio desenvolvedor (s2mpletista)
- Mensagens curtas, imperativas: "feat: add TUI mode", "fix: handle TLS error"
- **NUNCA** use `gh` CLI — use `curl` com o token do GitHub fornecido pelo usuário
- Antes de tag/push: `git log --all --format='%h %s%n%b' | grep -iE "co-authored|claude|anthropic"` para garantir corpo limpo

## 2. Testes e Qualidade
- **SEMPRE** teste antes de commit: `cargo test`, `cargo build --release`, `cargo clippy`, `cargo fmt --check`
- CI roda: fmt + clippy + test + release build — tudo deve passar
- Zero warnings de clippy/fmt em CI

## 3. Estilo de Código
- Código **limpo, idiomático Rust** — sem comentários verbosos "estilo IA"
- Comentários só onde necessário (lógica complexa, unsafe, decisões não-óbvias)
- `rustfmt` padrão — rode `cargo fmt` antes de commit
- Nomes claros, evite abreviações desnecessárias

## 4. Arquitetura do Projeto
- **um binário, poucos flags** — filosofia do auger
- 5 comandos: `run`, `scan`, `check`, `cert`, `ping` + `report`, `html`, `compare`
- TLS via `rustls` + `ring` (sem openssl — build cross aarch64-linux)
- `reqwest` com `rustls-tls`, `http2`, `charset`, `macos-system-configuration`

## 5. Release Process
- Version bump no `Cargo.toml` + `CHANGELOG.md` + `README.md` se necessário
- Tag `vX.Y.Z` → push → GitHub Actions `release.yml` builda 5 targets:
  - x86_64-pc-windows-msvc
  - x86_64-unknown-linux-gnu
  - aarch64-unknown-linux-gnu
  - x86_64-apple-darwin (macos-13)
  - aarch64-apple-darwin (macos-14)
- Binários anexados ao release + `SHA256SUMS`

## 6. Memória Persistente (em ~/.claude/projects/D--auger/memory/)
- `no-ai-attribution.md` — regra de ouro
- `fmt-commit-all.md` — após `cargo fmt`, commit cada arquivo reformatado
- `session-end-save.md` — salvar pontos importantes ao final da sessão
- `competitors.md` — cenário competitivo (oha, feroxbuster, etc.)
- `project-state.md` — histórico completo do projeto

## 7. Contexto Atual (Agosto 2026)
- **v0.3.0 publicada** — 44 downloads crates.io, 0 estrelas GitHub
- **v0.4.0 em desenvolvimento** — feature principal: `--tui` (dashboard tempo real no `auger run`)
- TUI usa `ratatui` + `crossterm` behind feature flag `tui`
- Build com `--features tui` compila OK
- Próximos passos: testar TUI, atualizar README/CHANGELOG, bump versão, release

## 8. Perfil do Usuário
- s2mpletista, dev Rust brasileiro, escreve em português
- Não quer atribuição de IA em nada
- Prefere commits pequenos, diretos, sem floreios
- Token GitHub disponível via `git credential fill` (não está no código)

---

**Resumo para próxima IA**: "Trabalhe no auger (CLI Rust load test/discovery). Regras: zero atribuição IA, commits secos como dono do repo, teste antes de commit, não use gh CLI, use token fornecido. Projeto em v0.4.0 adicionando TUI mode. Memória em ~/.claude/projects/D--auger/memory/."