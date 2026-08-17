# 03 — Protocolo

## Princípio

**Só inputs trafegam.** Nunca estado, nunca posições, nunca "o oponente está
em (x, y)". Os dois peers rodam a mesma simulação e chegam à mesma conclusão
sozinhos. É isso que permite ~35 kbit/s por direção para um jogo de luta a 60 Hz.

## Formato do datagrama

```
offset  tam  campo
     0    2  magic "RB"
     2    1  versão do protocolo
     3    1  tipo de mensagem
     4    4  sequência (u32 LE, por remetente, monotônica)
     8    n  payload
   8+n   32  HMAC-SHA256 sobre os bytes [0, 8+n)
```

Decisões e motivos:

- **Tudo little-endian de largura fixa.** Sem varints e sem prefixos de tamanho
  que possam discordar do buffer real: o decodificador consome o payload
  exatamente e **rejeita bytes sobrando** (`WireError::TrailingBytes`).
- **A versão está dentro da região autenticada.** Um atacante não consegue
  rebaixar um peer para um parser antigo virando o byte 2.
- **Limite duro de 1200 bytes.** Fica abaixo da MTU Ethernet típica de 1500
  menos cabeçalhos IPv6/UDP e uma folga de túnel, então um datagrama **nunca
  fragmenta**. UDP fragmentado significa que perder um fragmento mata o
  datagrama inteiro — exatamente o modo de falha que uma sessão de rollback
  menos pode pagar.
- **Valores de enum desconhecidos são rejeitados, não tratados como padrão.**
  Um `DisconnectReason` de valor 200 é erro de parse, não "normal".

Os bytes exatos de cada mensagem estão fixados em
`crates/rollback-net/tests/golden_protocol.rs`. Se um vetor daquele arquivo
mudar, o formato mudou, e `PROTOCOL_VERSION` precisa mudar junto.

## As seis mensagens

| Tipo | Código | Conteúdo |
|---|---|---|
| `Hello` | 1 | identidade do peer |
| `HelloAck` | 2 | identidade + motivo de recusa (0 = aceito) |
| `InputBatch` | 3 | frame inicial, até 8 inputs, maior frame remoto recebido, ACK |
| `Checksum` | 4 | frame + checksum do estado |
| `TelemetrySummary` | 5 | 18 contadores do próprio peer |
| `Disconnect` | 6 | motivo |

### `InputBatch`: a estratégia inteira de recuperação de perda

Não há retransmissão. Não há ACK por frame. Não há janela deslizante.

Cada `InputBatch` repete **os últimos oito inputs locais**. A 60 Hz, um input
tem oito chances de chegar antes de o peer precisar dele. Com 2% de perda
independente, a probabilidade de perder as oito é 0,02⁸ ≈ 2,6 × 10⁻¹⁴.

Isso é mais simples e mais rápido que retransmissão: um input perdido é
recuperado pelo datagrama seguinte, 16,7 ms depois, sem nenhuma negociação. E é
o motivo de o `InputBatch` ser idempotente por construção — reinserir um valor
já conhecido é no-op, e o teste `repeated_delivery_is_idempotent` prova isso
para qualquer sequência.

O batch também carrega:

- `highest_remote_frame` — até onde o remetente já confirmou nossos inputs, útil
  para diagnóstico;
- `ack_sequence` — a maior sequência que o remetente viu de nós, que é como o
  RTT é amostrado (ver abaixo).

### `Checksum`: detecção de desync

A cada 60 frames confirmados, cada peer manda o checksum do estado no início
daquele frame. O recebedor compara.

A verificação é **adiada**, não imediata. Duas condições precisam valer antes
que a comparação signifique algo:

1. já simulamos aquele frame, e
2. nosso próprio estado naquele frame é **final** — todos os inputs anteriores
   confirmados.

Nenhuma das duas é garantida na hora da chegada. O peer manda o checksum assim
que o frame é final *para ele*, e quem estiver rodando alguns milissegundos à
frente chega lá primeiro. Comparar cedo demais produziria um falso desync contra
um estado que um rollback pendente está prestes a reescrever; e descartar o que
chegou cedo demais faria a detecção funcionar em uma direção só.

> Esse segundo problema foi encontrado pelo teste E2E: com o perfil `natural`,
> um peer comparava 10 checksums e o outro comparava 0. Os checksums agora ficam
> estacionados até as duas condições valerem.

Um desacordo confirmado **encerra a sessão imediatamente**. Não há recuperação:
os dois jogos já são diferentes.

## Autenticação

Todo datagrama carrega um HMAC-SHA256 sobre o corpo inteiro.

UDP não tem estado de conexão. Sem isso, qualquer um que adivinhe a porta pode
injetar um frame de input na partida de alguém — ou, pior, um `InputBatch` que
contradiz um frame confirmado e mata a sessão.

O que isso **não** faz:

- **Não é criptografia.** Inputs não são segredo.
- **Não é defesa contra replay por si só.** A contabilidade de frames da própria
  sessão torna um batch repetido um no-op.

Ele responde exatamente uma pergunta: *foi o peer que tem a chave da sessão que
mandou isto?*

A verificação usa comparação em tempo constante (`Mac::verify_slice`). Uma
comparação byte a byte com retorno antecipado vazaria o tag um byte por vez para
quem consiga medir o tempo das respostas.

### A chave

- 32 bytes, gerada por sessão a partir de `/dev/urandom`.
- Guardada em SSM Parameter Store como `SecureString`.
- **Nunca entra no estado do Terraform** — o recurso é criado com um placeholder
  e tem `ignore_changes = [value]`; quem escreve o valor real é `just aws-up`.
- **Nunca aparece em linha de comando**, porque um argumento é visível no `ps`
  para qualquer usuário da máquina. Ela vem de `ROLLBACK_SESSION_KEY` ou de
  `ROLLBACK_SESSION_KEY_FILE` (modo 0600).
- O `Debug` de `Authenticator` imprime `<redacted>`, e há um teste que garante
  que nenhum byte da chave escapa por ali.

## Handshake

O objetivo do handshake **não é segurança** — o HMAC já respondeu "é o peer
certo?". É **compatibilidade**.

`PeerIdentity` carrega tudo que, se diferisse entre os peers, faria as simulações
divergirem:

| Campo | Por quê |
|---|---|
| versão do protocolo | parsers diferentes |
| simulação (arena/sfa3) | jogos diferentes |
| commit do app | builds diferentes podem simular diferente |
| hash da configuração | input delay, limite de previsão, histórico, seed |
| seed | semeia os bots |
| SHA-256 do core | emulador diferente = simulação diferente |
| SHA-256 da ROM | revisão de ROM diferente = jogo diferente |
| slot do jogador | os dois não podem ser P1 |

A verificação é **ordenada**, então a recusa nomeia a *primeira* coisa que
difere: "ROM hash mismatch" é infinitamente mais útil que "incompatível".

Os dois lados verificam independentemente, em vez de o cliente confiar no ack do
host. E os dois mandam `Disconnect` antes de desistir, para que a outra ponta
receba um motivo em vez de um timeout.

Nem o core nem a ROM são transmitidos — só os digests de 32 bytes. Um peer
descobre se o outro tem o mesmo arquivo sem que o laboratório distribua nada.

## Emulação de rede

O atraso, jitter, perda, duplicação e reordenação sintéticos são aplicados aos
datagramas de **saída**, não de entrada.

Atrasar a entrada seria mais fácil, mas não reproduziria o fenômeno que
interessa: sob perda real, os dados do remetente nunca existem no cabo, e é a
redundância do `InputBatch` dele que precisa cobrir isso. Impedir a saída coloca
o emulador na mesma posição da rede real.

Consequência: um experimento simétrico significa o **mesmo perfil configurado
nos dois lados**, e o RTT vê aproximadamente o dobro do atraso unidirecional
configurado. Os números do relatório refletem isso.

O reordenamento adiciona 25 ms extras ao datagrama sorteado — precisa exceder o
intervalo entre pacotes (16,7 ms a 60 Hz), senão ele chegaria em ordem mesmo
assim e o perfil não faria nada.

Tudo é semeado (`NetworkProfile::seed`), então um experimento é repetível.

## Medição

| Métrica | Como |
|---|---|
| RTT suavizado (SRTT) | RFC 6298, a partir do `ack_sequence` dos `InputBatch` |
| Variação do RTT (RTTVAR) | RFC 6298 |
| Perda inferida | lacunas na sequência do peer: `(maior_seq + 1) − únicos` |
| Duplicação | janela de 64 sequências, bitmask |
| Reordenação | sequência menor que a maior já vista |
| Bitrate | bytes × 8 ÷ tempo decorrido |

### Por que não há latência unidirecional

Medir atraso one-way exige que os dois relógios concordem. Duas máquinas em
lados opostos do Atlântico com NTP não sincronizado facilmente diferem em dezenas
de milissegundos — a mesma ordem de grandeza da coisa sendo medida.

Reportar `chegada − envio` entre esses relógios produziria um número que parece
preciso e não significa nada. O laboratório reporta RTT (que precisa de um
relógio só) e **não diz nada** sobre latência unidirecional. Isso está escrito
no topo de `crates/rollback-net/src/link.rs` e repetido nas ressalvas do
relatório HTML.

### Perda inferida se auto-corrige

Um datagrama atrasado aparece como perda até chegar. Quando chega, o contador de
únicos sobe e a estimativa se corrige sozinha. É por isso que a métrica é
chamada de *inferida* e não de *medida*.

## O que não existe no MVP

STUN, relay, matchmaking, espectador, reconexão e sincronização de estado. Um
caminho direto entre os peers é **pré-condição**, não algo que o laboratório
negocia. Se a EC2 não estiver alcançável em UDP/7000 a partir do seu IP, a
sessão não começa — e o handshake dirá isso com um timeout.
