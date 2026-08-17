# 12 — Troubleshooting

## O handshake expira

```
Error: handshake failed
Caused by: no compatible peer answered within 60s
```

Em ordem de probabilidade:

1. **Chaves de sessão diferentes.** Todo datagrama falha o HMAC e é descartado
   *antes* de virar mensagem, então o sintoma é silêncio, não recusa. Confirme:
   `curl -s http://127.0.0.1:9898/metrics | grep auth_failures` — se estiver
   subindo, alguém está falando, mas com a chave errada.
2. **O `allowed_cidr` não é o seu IP atual.** IP residencial muda.
   `curl -s https://checkip.amazonaws.com` e compare com
   `terraform -chdir=terraform output allowed_cidr`.
3. **UDP bloqueado no caminho.** Alguns provedores e redes corporativas
   descartam UDP em portas altas. Teste com `nc -u <ip> 7000` dos dois lados.
4. **O peer remoto não está rodando.**
   `aws ssm start-session --target <id>` e `systemctl status rollback-bot`.

## O handshake é recusado com um motivo

```
Error: peer refused the session: ROM hash mismatch
```

Isso é o sistema funcionando. A lista de motivos possíveis, e o que fazer:

| Motivo | Causa | Correção |
|---|---|---|
| `protocol version mismatch` | builds de versões incompatíveis do protocolo | recompile os dois lados |
| `peers chose different simulations` | um `--sim arena`, outro `--sim sfa3` | use o mesmo |
| `peers run different application builds` | commits diferentes | `just aws-up` de novo, do mesmo commit |
| `session configuration mismatch` | `--input-delay` ou `--prediction-limit` diferentes | use os mesmos valores |
| `session seed mismatch` | `--seed` diferente | use a mesma semente |
| `libretro core hash mismatch` | cores compilados diferentes | copie o mesmo `.so` para os dois |
| `ROM hash mismatch` | revisões de ROM diferentes | use exatamente o mesmo arquivo |
| `both peers asked for the same player slot` | dois `--player p1` | um deve ser `p2` |

## A sessão trava (faixa cinza contínua)

Cinza no overlay = stall: a janela de previsão encheu e a simulação parou.

- **Ocasional sob jitter alto:** normal. É o limite de previsão fazendo o trabalho
  dele, impedindo que um rollback precise voltar mais fundo que o buffer alcança.
- **Contínuo:** o peer parou de falar. Depois de 3 segundos sem datagrama
  autenticado, a sessão encerra com `PeerTimeout`.

Olhe `rollback_inferred_lost_total` e `rollback_srtt_seconds`. Se o RTT
disparou, a rede piorou. Se a perda foi a 100%, o caminho caiu.

## Desync confirmado

```
Error: session ended in a confirmed desync
```

Isso significa que as duas simulações divergiram. O processo de diagnóstico
completo está em [05 — Determinismo](05-determinismo.md); em resumo:

```bash
# qual frame, e quais foram os dois checksums
jq 'select(.event=="desync")' artifacts/logs/*.jsonl

# os dois peers rodavam o mesmo commit?
jq -r 'select(.record=="session_start") | .info.app_commit' artifacts/logs/*.jsonl
```

Depois: reproduza com `just bench` na mesma semente e perfil; tente debug contra
release; tente na arena antes do SFA3.

## `effective_fps` bem abaixo de 60

O peer não está conseguindo simular a 60 Hz.

```bash
# quanto do orçamento de 16,7 ms está sendo usado
curl -s http://127.0.0.1:9898/metrics | grep -E "advance_seconds|save_state_seconds"
```

Para o SFA3 na EC2, a causa usual é `retro_serialize` num `t3.small`. Troque para
`t3.medium` em `terraform.tfvars` e rode `just aws-up` de novo.

Localmente, verifique se não está rodando em build de debug — o `justfile` usa
release, mas um `cargo run` manual sem `--release` é 10× mais lento.

## O Prometheus não coleta nada

```
http://127.0.0.1:9898/metrics  down
```

- **Nenhuma sessão rodando.** O exportador só existe enquanto há uma sessão. É o
  estado normal entre execuções.
- **A stack não está em rede de host.** O `docker-compose` usa `network_mode: host`
  de propósito, para alcançar um exportador de loopback. Se você mudou para bridge,
  ele não alcança — ver [06 — AWS](06-aws.md) para o motivo de não abrir o
  exportador em `0.0.0.0`.
- **Não é Linux.** Rede de host no Docker é Linux-only.

## O cliente SDL não abre

```bash
echo $XDG_SESSION_TYPE     # esperado: wayland ou x11
```

Em TTY puro não há compositor e o SDL não tem onde desenhar. O `rollback-bot`
(headless) funciona nessas condições e é o que os testes automatizados usam.

Se o gamepad não aparece, não é erro: o teclado é um dispositivo de entrada
completo sozinho. O cliente imprime `gamepad: <nome>` quando encontra um.

## `LocalInputRefiled`

```
Error: local input for frame 1234 was queued twice with different values
```

Bug no laço do chamador: ele enfileirou um input local durante um stall. O
`SessionRunner` checa `would_stall()` antes de ler o controle exatamente para
evitar isso. Se aparecer, é um laço customizado que pulou essa checagem.

## `PeerContradiction`

```
Error: peer sent two different inputs for frame 1234
```

O peer mandou dois valores diferentes para o mesmo frame. Isso não é artefato de
rede — duplicação e reordenação são absorvidas silenciosamente. É um peer com
bug, ou um datagrama forjado que passou pelo HMAC (o que significaria vazamento
da chave).

## `HistoryExhausted`

```
Error: cannot roll back to frame 1200: oldest saved state is 1208
```

Um rollback precisou voltar mais fundo que o buffer de estados alcança. Não
deveria acontecer: `SessionConfig::validate` exige `state_history > prediction_limit`.

Se aparecer, ou a configuração foi construída contornando o validador, ou há um
bug na contabilidade da profundidade de previsão. Foi exatamente esse erro que os
testes de propriedade produziram ao encontrar o bug da condição de stall.

## `terraform apply` falha

- **`InvalidClientTokenId`** — credenciais AWS erradas ou expiradas.
  `aws sts get-caller-identity`.
- **`UnauthorizedOperation`** — falta permissão IAM. O laboratório precisa de
  EC2, VPC, S3, IAM e SSM.
- **`AddressLimitExceeded`** — limite de Elastic IPs na região. Provavelmente há
  EIPs órfãos de uma execução anterior; ver [11 — Cleanup](11-cleanup.md).
- **`terraform.tfvars is missing`** — copie de `example.tfvars`.

## O build do FBNeo falha

```bash
just build-core
```

- **`No rule to make target`** — o commit pinado não tem `src/burner/libretro`.
  Esse é exatamente o problema descrito em [09 — SFA3](09-sfa3.md): o port
  libretro vive no fork `libretro/FBNeo`, não no upstream.
- **Sem espaço** — a build precisa de ~5 GB. `docker system prune`.
- **Lento** — 20 a 40 minutos numa máquina de 10 núcleos é esperado. Ajuste com
  `JOBS=n`.

## `core reports a serialize size of zero`

```
Error: loading ROM "/caminho/sfa3.zip"
Caused by: core reports a serialize size of zero, so no game is actually running.
```

O core carregou, o ROM foi aceito, e nenhum jogo está rodando. O FBNeo retorna
sucesso de `retro_load_game` mesmo com um romset inutilizável — ele mostra o
motivo na tela emulada em vez de contar ao frontend — então um estado de tamanho
zero é como um set incompleto aparece aqui.

Causa mais comum: **falta um arquivo**. Sets CPS-2 atuais incluem a chave de
descriptografia dentro do zip (para o SFA3: `sfa3.key`, 20 bytes, CRC
`54fa39c6`); um set antigo tem os outros 20 arquivos corretos e mesmo assim não
roda.

```bash
just inspect-core rom=/caminho/sfa3.zip
```

Isso imprime o que o core diz, quais comandos de ambiente ele pediu, e o tamanho
do estado. Para conferir arquivo por arquivo, compare os CRCs do seu zip com
`Sfa3RomDesc[]` em `src/burn/drv/capcom/d_cps2.cpp` no FBNeo.

## O relatório sai vazio

```
0 sessão(ões) lidas de artifacts/logs
```

Nenhum `.jsonl` no diretório. Rode `just bench` ou `just collect` primeiro.

Se houver arquivos mas eles aparecerem como incompletos, a sessão morreu antes do
registro `session_end`. O relatório ainda usa o que chegou e marca
`complete=false` — veja o final do arquivo para o último estado registrado.

## Como pedir ajuda de forma útil

Junte:

```bash
just test 2>&1 | tail -40
jq -c 'select(.record=="session_start" or .record=="session_end")' artifacts/logs/*.jsonl
curl -s http://127.0.0.1:9898/metrics | grep -v '^#'
git rev-parse HEAD
```

Os dois `session_start` mostram se os peers concordavam sobre commit, semente e
configuração — que responde a maior parte das perguntas antes de serem feitas.
