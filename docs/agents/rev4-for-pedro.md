# Pedro: confirmar as mudanças de padrão do rev 4 (D-013)

> **Confirmado:** Pedro confirmou os 15 itens em 2026-11-24 (D-157).

Este é um resumo para você decidir. O texto completo está em `docs/contracts/m1-policy-rev4.md`.

O rodrigo aprovou o texto (D-136), e o Codex aprovou na quinta rodada. Falta a sua confirmação dos itens da §11, porque mudam padrões de segurança. As tarefas P0 a P5 só começam depois dela.

## Por que

O lote M1-27 a M1-32 teve 18 revisões do Codex, todas com rework, e cada rodada achava uma variante vizinha da anterior. A causa estava nos objetivos. Vários itens pediam que o motor entendesse com exatidão shell dinâmico (K7 `sh -c '"$1"'`, K9, `$'…'`) ou raciocinasse sobre padrões de glob contra o conjunto protegido. Shell é grande demais para isso.

## A ideia

O motor só promete exatidão dentro de um modelo pequeno e fechado: palavras literais, separadores simples, uma lista curta de wrappers, `sh -c` com script literal e uma tabela única de ferramentas. O que fica fora do modelo vira **"não lido"**:
- **No Home e com regras de prefixo ou exatas,** pede aprovação.
- **Com regras "Any" sem escopo e no FullAccess,** é liberado depois da verificação de melhor esforço que o `dev` já faz.

Pedir aprovação também no FullAccess foi descartado por dois motivos:
- **Uso real:** 53% de 3.353 comandos reais de agentes têm alguma construção dinâmica (`$VAR`, loops, `$(…)`, heredoc).
- **Contorno trivial:** no FullAccess o hard deny não segura quem quer contorná-lo. Basta gravar `sudo id` num arquivo e rodar `sh x.sh`.

## Os 15 itens para confirmar (§11)

1. **"Não lido" é liberado sob Any e FullAccess,** depois do melhor esforço. Isso generaliza o segundo ponto do D-133.
2. **"Não lido" sempre pede aprovação no Home e com regras de prefixo ou exatas,** inclusive em linhas que o `dev` libera resolvendo-as: K7, linhas longas de `env` (K9) e palavras ANSI-C.
3. **O texto em Settings,** com as premissas declaradas: nomes de comandos, configuração do usuário e do repositório, conteúdo de objetos git, outros processos, hard links e descritores herdados.
4. **Regras de prefixo que terminam num executor de código** (`["awk"]`, `["python"]`, `["pipx","run"]`, `["git"]`) passam a ser inválidas na criação.
5. **Regras já salvas que a nova validação rejeita** deixam de cobrir qualquer coisa, mas continuam listadas e marcadas como inválidas.
6. **Uma linha que escreve e depois lê** passa a pedir aprovação com regras de prefixo ou exatas (`ln -s … x; cat x`).
7. **Hard links dentro de raízes recursivas** ficam fora da promessa além da varredura limitada que o `dev` já faz.
8. **Uma regra que nomeia o programa** (`python build.py`) cobre o conteúdo dele.
9. **Operandos de padrão, texto, ref e membro** de ferramentas conhecidas deixam de contar como caminho. Assim, `grep .npmrc README.md` deixa de ser bloqueado.
10. **Novas partes "não lidas"** que o `dev` libera com regras:
    - matches de glob que só existem numa variante do shell;
    - cópias recursivas e extrações de arquivo compactado;
    - wrappers fora da lista;
    - globs na palavra de comando;
    - `&>`;
    - descritores além de 0, 1 e 2;
    - pipelines com um comando que escreve;
    - pathspecs git com curinga.
11. **Varreduras do sistema de arquivos que batem no limite** viram "não lido". Pedem aprovação no Home e com regras, e são liberadas sob Any e FullAccess. No `dev`, alguns estouros pedem aprovação em todo lugar.
12. **O S8** (hard deny de pastas que contêm uma credencial), com as exceções da home, dos ancestrais da home e de `~/.config`, e estendido aos ancestrais das pastas de dados do Atrium fora da home.
13. **Nomes que só batem com um nome protegido por outra equivalência** (normalização Unicode, outras dobras de maiúsculas) viram "não lido": o Home pede aprovação onde o `dev` libera.
14. **Caminhos que dependem do processo** (`/proc/self/…`, `/dev/fd/…`, `/dev/std*`) viram "não lido".
15. **Buscas que seguem links podem ser permitidas no Home** (`rg -L`, `grep -R`, `find -L`).
    - **O que muda:** se o Gate percorre os links da pasta e, dentro do orçamento, nenhum leva a algo protegido, a busca é permitida. No `dev`, o Home sempre pede aprovação para essas buscas.
    - **O que continua igual:** um link que chega a algo protegido continua bloqueado, e um percurso que estoura o limite pede aprovação.
    - **Por que entrou agora:** a linha 15 dos exemplos do contrato já dizia isso, mas faltava o item no §11. rodrigo aprovou em 2026-11-22 (D-139).

**Mantido de propósito:**
- `Unknown` pede aprovação em todo lugar.
- Os limites de tamanho continuam dando Deny.
- Orçamento esgotado pede aprovação em todo lugar.
- As proteções do Home e o S8 continuam.
- Probes de credenciais, tanto as de hard deny quanto as que pedem aprovação.
- O Ask para arquivos com mais de um hard link.

## O que você decide

- Confirmar os 15 itens.
- Ou recusar itens específicos, cada um com o que fazer no lugar.

O item 1 é o central. Se você o recusar, a alternativa é pedir aprovação também no FullAccess para linhas dinâmicas, o que, pela medição, pediria em cerca de metade dos comandos.
