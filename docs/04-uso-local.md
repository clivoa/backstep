# 04 — Uso local

## Controles

Teclado e gamepad são lidos **os dois** a cada frame e combinados com OR, então
dá para segurar a direção no manche e apertar botão no teclado.

| Ação | Teclado | Gamepad | SFA3 |
|---|---|---|---|
| Cima | `W` / `↑` | D-pad ↑, manche esquerdo | pular |
| Baixo | `S` / `↓` | D-pad ↓ | abaixar |
| Esquerda | `A` / `←` | D-pad ← | — |
| Direita | `D` / `→` | D-pad → | — |
| Ataque | `J` | `X` | soco fraco |
| Defesa | `K` | `A` | chute fraco |
| Especial | `L` | `Y` | soco médio |
| Confirmar | `U` | `B` | chute médio |
| Start | `Enter` | `Start` | start |
| Ficha | `Espaço` | `Back/Select` | inserir ficha |
| Sair | `Esc` | — | — |

Detalhes deliberados:

- **A leitura é do estado *segurado*, não de eventos.** Rollback precisa do
  input tal como estava numa fronteira de frame específica; uma fila de eventos
  reporta o que aconteceu em algum ponto entre dois frames.
- **Direções opostas se cancelam** (SOCD → neutro). Um manche de arcade
  fisicamente não consegue reportar esquerda e direita ao mesmo tempo; manter
  essa garantia aqui significa que a simulação nunca precisa definir o que
  "ambos" quer dizer, e os dois peers não podem discordar sobre isso.
- **A zona morta do analógico é grande** (metade do curso). Um manche gasto que
  registra uma direção fantasma vira uma previsão errada no peer, que é uma forma
  bem confusa de descobrir que o seu hardware está acabando.

## Comandos

```
just                 lista tudo
just test            fmt + clippy + testes (debug e release) + shellcheck + terraform
just e2e             dois processos, socket real, os cinco perfis
just bench           180 s por perfil, bot contra bot, gera o relatório
just local-up        Prometheus + Grafana em 127.0.0.1
just local-down      derruba a stack
just play sim=…      humano no P1 contra o peer remoto
just report          reconstrói o relatório a partir dos logs em disco
just build-core      compila o FBNeo em container reproduzível
just clean-logs      apaga logs e relatórios locais (não toca na AWS)
```

## Uma sessão local ponta a ponta, sem AWS

Útil para desenvolver: dois processos na mesma máquina, ligados por loopback.

Terminal 1 — o peer que hospeda (faz o papel da EC2):

```bash
export ROLLBACK_SESSION_KEY=$(head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
cargo run --release -p rollback-bot -- \
    --sim arena --player p2 \
    --bind 127.0.0.1:7000 \
    --profile combined --duration 180 \
    --metrics 127.0.0.1:9899
```

Terminal 2 — você:

```bash
export ROLLBACK_SESSION_KEY=<a mesma chave do terminal 1>
cargo run --release -p rollback-client -- \
    --sim arena --peer 127.0.0.1:7000 \
    --profile combined
```

A chave precisa ser **a mesma** nos dois lados, senão todo datagrama falha o HMAC
e o handshake expira. Isso é o comportamento correto, mas o sintoma
(`no compatible peer answered`) não é óbvio — veja
[12 — Troubleshooting](12-troubleshooting.md).

## O overlay

O cliente desenha, no canto, o que o netcode está fazendo:

```
FRAME 1234 CONFIRMED 1230 AHEAD 3
PREDICTED 4021 WRONG 260 ACC 94%
ROLLBACKS 260 DEPTH 3 MAX 6
RESIM 812 STALLS 0 STATE 204B
RTT 41MS VAR 7MS LOSS 2%
SENT 1234 RECV 1210 DUP 4 REORD 11
PROFILE COMBINED
```

E, na base da tela, uma faixa com os últimos 180 frames, um pixel por frame:

| Cor | Significa |
|---|---|
| Verde | **Confirmado** — os dois inputs eram conhecidos, nada foi chutado |
| Amarelo | **Previsto** — o input remoto foi adivinhado |
| Vermelho | **Corrigido** — um rollback aconteceu neste frame |
| Cinza | **Travado** — a janela de previsão encheu e a simulação esperou |

Um detalhe da faixa: um rollback é pintado no frame em que foi **percebido**, não
nos frames que ele de fato re-simulou. Aqueles já saíram de tela, e reescrever a
história ali esconderia *o quanto* a correção foi tardia — que é justamente a
parte interessante.

Numa sessão saudável, a faixa é quase toda verde e amarela com vermelhos
esparsos. Cinza contínuo significa que o peer parou de falar.

## O que esperar de cada perfil

Rodando `just bench`, os perfis produzem comportamentos bem distintos:

- **`natural`** — sem impedimento. Em loopback quase nada é previsto: o input
  remoto chega antes do frame a que pertence.
- **`delay20`** — 20 ms por direção. O peer fica ~2,5 frames atrás, então quase
  todo frame é previsto e as correções aparecem.
- **`jitter30`** — 30 ± 15 ms. A profundidade de previsão oscila; é o perfil que
  mais exercita a variação do RTT.
- **`loss2`** — 2% de perda, sem atraso. A redundância de 8 inputs absorve quase
  tudo; o interessante é ver a perda inferida subir sem os rollbacks
  acompanharem.
- **`combined`** — 40 ± 20 ms, 2% de perda, 0,5% de reordenação. O caso adverso.

## Onde ficam os artefatos

```
artifacts/
├─ logs/          um .jsonl por sessão, de cada peer
├─ report/        summary.csv e report.html
├─ system/        diretório de NVRAM do FBNeo (precisa ser idêntico nos peers)
└─ session.key    chave efêmera, modo 0600, apagada por `just aws-down`
```

Nada em `artifacts/` é versionado.

## O log JSONL

Um objeto JSON por linha. JSONL em vez de um documento único porque uma sessão
que morre no meio ainda deixa um arquivo legível até a última linha gravada — que
é exatamente quando o log importa.

Tipos de registro: `session_start`, `local_input`, `remote_inputs`, `sent`,
`received`, `session` (eventos do motor: advanced, stalled, rolled_back,
checksum_matched, desync), `metrics` (snapshot completo a cada 60 frames) e
`session_end`.

Para olhar rapidamente:

```bash
# quantos rollbacks e de que profundidade
jq -r 'select(.event=="rolled_back") | .depth' artifacts/logs/*-p1-bench.jsonl \
  | sort -n | uniq -c

# a linha final, com todos os contadores
jq 'select(.record=="session_end")' artifacts/logs/*-p1-bench.jsonl
```

## Wayland, SDL2 e o `--sim sfa3`

O cliente usa SDL2 sem vsync: a **sessão**, não o monitor, é dona do relógio de
frame. Um monitor de 144 Hz não pode fazer a simulação rodar a 144 Hz.

Se a janela não abrir, confira `XDG_SESSION_TYPE`. Em um TTY puro não há
compositor e o SDL não tem onde desenhar; o `rollback-bot` (headless) funciona
igual nessas condições e é o que os testes automatizados usam.
