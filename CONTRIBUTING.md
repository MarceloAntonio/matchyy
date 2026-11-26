# Como contribuir

Que bom que você quer ajudar! Contribuições de todo tamanho são bem-vindas: relato de bug, ideia, documentação ou código.

## Relatando um bug

Abra uma [issue](https://github.com/MarceloAntonio/matchyy/issues) com:

- o que você esperava e o que aconteceu (um print ajuda muito)
- o terminal e a versão (`kitty --version`, por exemplo)
- o player de música e a saída de `playerctl metadata`
- a distribuição e se você usa PipeWire ou PulseAudio
- as opções que estava usando (`matchyy -mr -pk`, por exemplo)

## Sugerindo uma funcionalidade

Abra uma issue antes de escrever o código e conte qual problema a ideia resolve. Opções novas precisam vir **desligadas por padrão**: quem não usar a flag não pode notar nenhuma diferença.

## Enviando código

1. Faça um fork e crie um branch: `git checkout -b minha-mudanca`
2. Faça a mudança em `src/main.rs`, seguindo as regras do [AGENTS.md](AGENTS.md). Elas valem para pessoas também.
3. Rode a verificação:
   ```sh
   cargo fmt
   cargo clippy -- -D warnings
   cargo test
   ```
4. Se mudou algo visual, rode o matchyy num kitty com música tocando e anexe um print ao pull request.
5. Se adicionou uma opção, atualize `HELP`, a tabela do README e o teste `opcoes`.
6. Abra o pull request explicando o quê e por quê.

## Estilo

- Mudanças pequenas e focadas: um pull request, um assunto.
- Mensagens de commit no imperativo, curtas: `Adiciona --peaks`, `Corrige corte do título`.
- Textos para o usuário em português, com acentuação correta.

## Licença

Ao contribuir, você concorda que sua contribuição será licenciada sob a [Apache 2.0](LICENSE), a mesma do projeto.
