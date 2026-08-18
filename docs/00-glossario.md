# 00 — Glossário

Todo termo técnico que aparece neste repositório, explicado do zero, com o
número que este laboratório usa e o motivo de ele existir.

Se você está começando por aqui, leia as três primeiras seções na ordem. O resto
funciona como consulta.

**Índice**

- [A ideia central](#a-ideia-central)
- [Rollback: as peças](#rollback-as-peças)
- [Rede: o que estraga a conversa](#rede-o-que-estraga-a-conversa)
- [Os cinco perfis de rede](#os-cinco-perfis-de-rede)
- [As métricas que o laboratório reporta](#as-métricas-que-o-laboratório-reporta)
- [Emulação e libretro](#emulação-e-libretro)
- [Determinismo](#determinismo)
- [Protocolo e segurança](#protocolo-e-segurança)
- [Infraestrutura](#infraestrutura)

---

## A ideia central

### Frame

Um passo de simulação. Este laboratório roda a **60 frames por segundo**, ou
seja, um frame a cada **16,7 ms**. Esse é o orçamento: tudo que acontece num
frame — ler o controle, mandar pela rede, simular, desenhar — tem de caber em
16,7 ms, ou o jogo engasga.

O frame é também a unidade de tudo: inputs são carimbados por frame, estados são
salvos por frame, checksums são comparados por frame.

### Input

O que o jogador está apertando **naquele frame**. Aqui é um único `u16`: 16 bits,
um por botão. Cabe num registrador, copia sem alocar, e é a unidade que atravessa
a rede.

### Simulação

A função que recebe o estado atual mais os inputs dos dois jogadores e produz o
próximo estado. Neste projeto ela é uma *trait* com quatro métodos:
`save_state`, `load_state`, `advance_frame`, `checksum`.

Duas implementações: a **arena** (um jogo de luta 2D minúsculo, escrito aqui) e o
**libretro** (um emulador de arcade inteiro). O motor de rollback não sabe a
diferença — e esse é o ponto do laboratório.

### Netcode

O conjunto de decisões sobre *como dois computadores mantêm o mesmo jogo em
sincronia pela rede*. Não é "código de rede" no sentido de sockets: é a política
de o que fazer quando a informação chega tarde. As três famílias:

| Família | Como lida com o atraso | Preço |
|---|---|---|
| **Lockstep** | espera o input do oponente antes de simular | o jogo trava/atrasa quando a rede piora |
| **Delay-based** | adiciona atraso fixo no input local para "esconder" a rede | o seu próprio comando responde tarde, sempre |
| **Rollback** | adivinha o input do oponente e corrige depois | trabalho extra de CPU, e correções visíveis |

---

## Rollback: as peças

### Rollback (netcode)

Em uma frase: **não espere pelo oponente — chute o que ele fez, siga jogando, e
volte atrás para corrigir quando o input real chegar.**

O nome vem do "voltar atrás": ao descobrir que chutou errado no frame 100, o
jogo restaura o estado salvo do frame 100 e re-simula 100, 101, 102… até o
presente, agora com o input correto. Tudo isso dentro de um único frame de
tela, sem o jogador ver o replay.

O ganho é que **o seu comando responde na hora**. O preço é CPU e correções
ocasionalmente visíveis.

### Previsão (prediction)

O chute sobre o que o oponente fez num frame cujo input ainda não chegou.

A regra aqui é a mais simples possível: **repita o último input confirmado dele**.
Não é ingenuidade — é que inputs de jogo de luta são *segurados* por muitos
frames (você fica andando para frente, fica agachado bloqueando). Medido neste
laboratório: acerta ~93% dos frames que precisou chutar, tanto na arena quanto no
Last Blade 2.

### Frame confirmado

Um frame para o qual os inputs **dos dois** jogadores já chegaram. É o passado
que não muda mais. Tudo depois dele é especulação.

### Profundidade do rollback (rollback depth)

Quantos frames o jogo teve de re-simular numa correção. Profundidade 1 é
invisível; profundidade 8 no meio de uma troca de golpes dá para perceber.

### Input delay (atraso de input)

Atraso **deliberado** entre você apertar o botão e o jogo agir. Aqui o padrão é
**1 frame** (16,7 ms).

Parece contraproducente, mas compra algo: com 1 frame de atraso o seu input sai
pela rede um frame antes de ser necessário, o que dá ao oponente uma janela a
mais para recebê-lo. Cada frame de input delay é um frame a menos de previsão —
troca-se resposta por precisão. Zero é possível e faz o rollback trabalhar mais.

### Limite de previsão (prediction limit)

Quantos frames o jogo aceita adivinhar antes de desistir e parar. Aqui: **8
frames**, ~133 ms.

Existe porque a re-simulação custa CPU: corrigir 8 frames de uma vez é
razoável, corrigir 60 não caberia no orçamento de 16,7 ms.

### Stall (travada)

O que acontece quando a janela de previsão enche: a simulação **para** e espera.
Um stall ocasional é o limite fazendo o trabalho dele. Stalls contínuos
significam que o peer parou de falar.

No overlay do cliente, stall aparece como uma faixa cinza.

### Histórico de estados (state history)

Quantos estados salvos o jogo mantém para poder voltar. Aqui: **16**.

Tem de ser **estritamente maior** que o limite de previsão — se o rollback pode
voltar 8 frames, o estado de 8 frames atrás precisa ainda existir. O
`SessionConfig::validate` recusa qualquer configuração que viole isso; foi
exatamente esse erro (`HistoryExhausted`) que um teste de propriedade produziu ao
encontrar um bug real na contabilidade da previsão.

### Snapshot / savestate

Uma cópia do estado inteiro da simulação, para poder voltar a ele.

O tamanho é a diferença mais brutal entre as duas simulações deste laboratório:

| | arena | Last Blade 2 (emulado) |
|---|---|---|
| snapshot | **204 bytes** | **415 155 bytes** |

Salvar 204 bytes é um `memcpy` desprezível. Salvar 405 KB **sessenta vezes por
segundo**, mais uma vez por frame re-simulado, é a maior parte do custo de CPU de
uma sessão emulada.

### Re-simulação (resimulation)

Rodar de novo os frames entre o ponto de rollback e o presente. É o "trabalho
extra": se num segundo o jogo simulou 60 frames de tela mas re-simulou outros 30,
o overhead é 50%.

### `OutputMode`

A distinção entre "este frame vai para a tela" (`Present`) e "este frame é
re-simulação, jogue o vídeo e o áudio fora" (`Resimulate`).

Sem isso, um rollback de 8 frames mostraria os 8 frames num piscar e tocaria uma
salva de áudio. A regra de ouro: **`OutputMode` pode mudar a saída, nunca o
estado.** Se mudar o estado, os dois peers divergem.

### Desync (dessincronização)

As duas máquinas deixaram de rodar o mesmo jogo. A partir daí os dois jogadores
estão vendo partidas diferentes e nada mais faz sentido.

Rollback **não tolera** desync melhor que lockstep — ele *depende* de a
re-simulação reproduzir exatamente o que teria acontecido.

### Checksum

Um número curto que resume o estado da simulação, trocado periodicamente entre os
peers para detectar desync. Aqui é FNV-1a de 64 bits sobre o snapshot, comparado
a cada **60 frames** (1 s) e só em frames confirmados.

Detalhe que custou caro: no core emulado o checksum **ignora os primeiros 2 048
bytes** do savestate, porque o FBNeo recalcula (em vez de restaurar) ~20 bytes de
contadores de som e timer. Sem essa exclusão o detector acusava desync no
primeiro rollback de toda sessão. O raciocínio completo está em
[05 — Determinismo](05-determinismo.md).

---

## Rede: o que estraga a conversa

### Datagrama

Uma mensagem UDP. "Pacote" no uso comum. Aqui cada datagrama tem no máximo
**1 200 bytes**, escolhido para caber num MTU típico da Internet sem
fragmentação.

### UDP e por que não TCP

TCP garante entrega e ordem — e faz isso **esperando**. Se um pacote se perde,
tudo que veio depois fica parado na fila até a retransmissão chegar
(*head-of-line blocking*). Para um jogo a 60 Hz isso é o pior comportamento
possível: o input de 200 ms atrás não interessa mais, e esperar por ele congela o
presente.

UDP não garante nada e não espera. O jogo prefere um input perdido a um input
atrasado.

### Latência (latency)

Tempo que um datagrama leva de um lado ao outro. **Unidirecional** (one-way) é
de A até B; **RTT** é ida e volta.

Este laboratório **não reporta latência unidirecional**, e a razão é honesta:
medi-la exige que os dois relógios estejam sincronizados, e eles não estão. Só o
RTT é mensurável sem confiar em relógio alheio.

### RTT (round-trip time)

Ida e volta: mando algo, você responde, quanto tempo passou. É a única medida de
latência que um peer consegue fazer sozinho.

Nas medições, `natural` em loopback dá **16,6 ms** de RTT. Isso não é rede — é o
laço de frame: o peer só responde no frame seguinte, então 16,7 ms é o piso
imposto pelos 60 Hz.

### SRTT e RTTVAR

RTT bruto oscila muito. O protocolo usa as fórmulas do **RFC 6298** (as mesmas do
TCP) para suavizar:

- **SRTT** (*smoothed RTT*): média móvel do RTT.
- **RTTVAR** (*RTT variation*): quanto o RTT está variando — a medida de jitter
  do ponto de vista de quem está medindo.

### Jitter

**Variação** da latência. Não é o atraso em si, é o quanto ele muda de pacote
para pacote.

Um link com 50 ms constantes é previsível: dá para compensar. Um link que varia
entre 20 ms e 80 ms tem a *mesma média* e é muito pior, porque nunca se sabe
quando o próximo input chega.

De onde vem, na prática: filas em roteadores que enchem e esvaziam, Wi-Fi
disputando o meio, rotas que mudam, agendamento de CPU no host.

Por que jitter importa menos para rollback do que para lockstep:

- **Lockstep** precisa esperar o **pior caso**, senão trava. Jitter alto força
  uma margem grande, que é atraso para todo mundo o tempo todo.
- **Rollback** já corrige a cada frame. Um datagrama que chegou 15 ms tarde é
  corrigido pelo mesmo mecanismo que corrige um que chegou 20 ms tarde. Ele só
  precisa que o pior caso **caiba na janela de previsão**.

Medido aqui: `jitter30` (30 ± 15 ms) e `delay20` (20 ms fixos) produzem
praticamente o mesmo número de rollbacks, embora o RTTVAR suba de 0,5 ms para
~18 ms.

### Perda de pacotes (packet loss)

Datagramas que simplesmente não chegam. Roteador com fila cheia descarta; Wi-Fi
com interferência perde; um cabo ruim corrompe e o checksum de rede descarta.

Expressa em porcentagem. 2% significa que 1 em cada 50 datagramas some.

**Perda em rajada (burst loss)** é o caso realista e o pior: em vez de 1 pacote
perdido a cada 50, você perde 5 seguidos e depois nenhum por muito tempo. A
Internet real perde em rajada. O emulador deste laboratório perde de forma
independente, o que é mais gentil — está anotado como limitação.

### Redundância de input

A defesa contra perda, e o motivo de **não haver retransmissão** neste protocolo.

Cada datagrama carrega os **últimos 8 inputs**, não só o mais recente. Um input
perdido chega de novo no datagrama seguinte, 16,7 ms depois — muito antes de ser
necessário.

Custa quase nada (input é 2 bytes) e a matemática é convincente: com 2% de perda,
a chance de os oito datagramas que carregam um mesmo input se perderem é
0,02⁸ ≈ 2,6 × 10⁻¹⁴.

Efeito medido: `loss2` (2% de perda) produziu **4 rollbacks em 240 segundos**,
contra 1 006 do `delay20`. Perda quase não vira rollback. Perda e latência não
são o mesmo problema — e o rollback só é sensível ao segundo.

### Reordenação (reordering)

Pacotes que chegam fora de ordem, porque tomaram rotas diferentes. O protocolo
absorve silenciosamente: cada input traz o número do frame a que pertence, então
chegar fora de ordem é irrelevante.

### Duplicação

O mesmo datagrama entregue duas vezes. Também absorvido em silêncio — um input
repetido idêntico não muda nada.

O que **não** é absorvido é o mesmo frame com valores *diferentes*: isso é
`PeerContradiction`, e significa um peer com bug ou um datagrama forjado.

### Número de sequência

Um contador crescente em cada datagrama. Serve para duas coisas: medir RTT
(casando resposta com envio) e **inferir perda** a partir dos buracos na
sequência.

"Inferir" é a palavra certa: um datagrama atrasado aparece como perdido até
chegar.

---

## Os cinco perfis de rede

Um **perfil** é uma degradação de rede sintética que o laboratório injeta nos
datagramas de **saída** de cada peer. É a variável independente dos experimentos:
a rede real por baixo é loopback (praticamente nada), então o que se mede é o
efeito do perfil e mais nada.

> **Por que o RTT medido é o dobro do configurado:** o atraso é aplicado na
> saída de *cada* lado. Um datagrama sofre `delay_ms` ao sair de A, e a resposta
> sofre `delay_ms` de novo ao sair de B. Ida e volta = 2 × `delay_ms`. Isso é
> deliberado — é o que acontece num link real, onde os dois sentidos têm atraso.

| Perfil | Atraso | Jitter | Perda | Reordenação | RTT medido |
|---|---|---|---|---|---|
| `natural` | — | — | — | — | 16,6 ms |
| `delay20` | 20 ms | — | — | — | 70 ms |
| `jitter30` | 30 ms | ±15 ms | — | — | 84–88 ms |
| `loss2` | — | — | 2% | — | 27 ms |
| `combined` | 40 ms | ±20 ms | 2% | 0,5% | 97–105 ms |

### `natural` — o controle

Nenhuma degradação. Mede o que o laboratório faz quando a rede é perfeita, e
serve de linha de base para tudo o mais.

**Imita:** dois jogadores na mesma LAN, ou na mesma casa.

**Resultado:** zero rollbacks. Os inputs chegam antes de serem necessários, então
o rollback nunca precisa entrar em ação. Útil justamente por isso — provou que os
desyncs que apareciam nos outros perfis vinham do *rollback*, e não da simulação.

### `delay20` — latência limpa

20 ms de atraso constante em cada sentido. Sem variação, sem perda.

**Imita:** um link de fibra bom entre cidades próximas. Madri–Barcelona,
São Paulo–Rio.

**Isola:** o efeito puro da **distância**. Como não há jitter nem perda, tudo que
acontece aqui é consequência do atraso — é o perfil que mostra quanto trabalho o
rollback faz só por o oponente estar longe.

**Resultado:** 1 006 rollbacks em 240 s, profundidade média 6,5, 46% de trabalho
extra de CPU. E, crucialmente, **em um só dos dois peers** (ver
[assimetria](#por-que-os-rollbacks-são-assimétricos)).

### `jitter30` — latência instável

30 ms de atraso com variação uniforme de ±15 ms: cada datagrama sofre entre 15 e
45 ms.

**Imita:** Wi-Fi, rede móvel, ou um link congestionado. É o perfil mais parecido
com a Internet doméstica de verdade.

**Isola:** o efeito da **variação**. Comparar com `delay20` responde à pergunta
"jitter é pior que atraso?".

**Resultado:** praticamente igual ao `delay20` — 1 006 rollbacks, acurácia 93%.
O RTTVAR sobe de 0,5 ms para ~18 ms, como esperado, mas a profundidade máxima mal
se move. **Para rollback, jitter não é pior que atraso.** Para lockstep, seria.

### `loss2` — perda sem latência

2% dos datagramas descartados, sem atraso adicional.

**Imita:** Wi-Fi com interferência, ou um enlace com erros — situações onde o
pacote some mas o caminho é curto.

**Isola:** o efeito da **perda**, separado do atraso. Existe para testar
especificamente a redundância de oito inputs.

**Resultado:** 4 e 19 rollbacks em 240 s — praticamente nada. A redundância
funciona: o input perdido chega no datagrama seguinte, antes de fazer falta.
Este perfil é a prova de que **não é preciso retransmissão**.

### `combined` — tudo junto

40 ms de atraso, ±20 ms de jitter, 2% de perda, 0,5% de reordenação.

**Imita:** uma conexão ruim de verdade — intercontinental, ou móvel em movimento.
É o pior caso que o laboratório se propõe a aguentar.

**Serve para:** dimensionamento. É aqui que se descobre se o limite de previsão
de 8 frames e o buffer de 16 estados têm folga.

**Resultado:** 1 004 rollbacks, profundidade máxima 7, 39 stalls, 34% de trabalho
extra — e **zero desyncs**. Os stalls mostram que o limite de previsão está
sendo tocado, ou seja, o dimensionamento está sendo exercido de verdade e não com
folga confortável.

### Por que os rollbacks são assimétricos

O resultado mais contra-intuitivo dos experimentos: sob `delay20`, um peer fez
**1 006** rollbacks e o outro fez **zero**.

Isso não é bug. Os dois peers rodam relógios de frame independentes, então existe
uma diferença de fase fixa entre eles. Quem está **à frente** chega no frame N
antes de o input do oponente para o frame N existir — então prevê, e às vezes
corrige. Quem está **atrás** recebe o input antes de precisar dele e nunca chuta.

Consequência prática: **um dos dois jogadores paga essencialmente todo o custo de
CPU do rollback**, e qual deles é decidido por alguns milissegundos na largada.
Comparar "quantos rollbacks meu jogo faz" entre dois clientes não diz nada sobre
a qualidade da rede — diz quem começou primeiro.

O sinal de que os dois estão jogando o mesmo jogo não é a simetria dos rollbacks.
É `checksums_compared` subindo dos dois lados com `desync = false`.

### Como escolher um perfil

```bash
just bench 180 arena              # os cinco, 180 s cada
DURATION=60 PROFILES=combined ./ops/scripts/bench.sh   # só um
```

Os perfis são **semeados**: o gerador de aleatoriedade da degradação tem semente
fixa, então repetir o experimento com a mesma semente produz exatamente a mesma
sequência de perdas e atrasos. Isso é o que torna `just bench` um experimento e
não uma anedota.

---

## As métricas que o laboratório reporta

Todas saem em `artifacts/report/summary.csv` e no exportador Prometheus.

| Métrica | O que é | Como ler |
|---|---|---|
| `effective_fps` | frames de tela por segundo | tem de ficar em 60. Abaixo disso, o peer não está dando conta |
| `rollbacks` | quantas correções aconteceram | assimétrico por construção — ver acima |
| `mean_rollback_depth` | média de frames re-simulados por correção | 1 é invisível, 8 é perceptível |
| `max_rollback_depth` | pior caso | se encostar no limite de previsão, o dimensionamento está no limite |
| `prediction_accuracy` | fração dos chutes que estavam certos | ~93% é o esperado; é o número que justifica o rollback existir |
| `resimulation_overhead` | trabalho extra / trabalho útil | 46% significa que quase metade de um frame a mais foi simulada por frame |
| `stalls` | vezes que a janela de previsão encheu | ocasional é normal; contínuo é peer morto |
| `checksums_compared` | comparações de estado que **concordaram** | tem de crescer nos dois lados |
| `desync` | as simulações divergiram | `true` invalida a sessão inteira |
| `srtt_ms` / `rttvar_ms` | RTT suavizado e sua variação | ver [SRTT](#srtt-e-rttvar) |
| `loss_pct` | perda **inferida** por buracos na sequência | um datagrama atrasado conta como perdido até chegar |
| `state_bytes` | tamanho de um snapshot | 204 na arena, 415 155 no emulado |
| `cpu_seconds` | CPU consumida pela sessão | 34 s para 90 s de sessão ≈ 38% de um núcleo |

---

## Emulação e libretro

### Emulador

Um programa que finge ser outro hardware. O FBNeo finge ser uma placa de arcade
— CPU 68000, Z80, chip de som, memória de vídeo — executando o código original
do jogo instrução por instrução.

Para rollback isso é ótimo e terrível ao mesmo tempo: ótimo porque um emulador é
naturalmente uma máquina de estados; terrível porque o estado tem centenas de
kilobytes e salvar isso 60 vezes por segundo custa caro.

### libretro

Uma **API** que separa o emulador da interface. O emulador vira uma biblioteca
(um *core*) com funções de nome fixo; o programa que a usa é o *frontend*.

Este laboratório é um frontend. Ele carrega o core com `dlopen` e chama:

| Função libretro | Para quê |
|---|---|
| `retro_run` | avança um frame |
| `retro_serialize` | salva o estado (o snapshot do rollback) |
| `retro_unserialize` | restaura o estado (o "voltar atrás") |
| `retro_serialize_size` | tamanho do estado |

Essas quatro são *exatamente* o que a trait `Simulation` pede. É por isso que um
emulador de arcade cai dentro do mesmo motor de rollback que a arena de
brinquedo, sem o motor saber.

### Core

A biblioteca do emulador. Aqui: `fbneo_libretro.so`, ~90 MB, compilada em
container a partir de um commit fixado.

Cores libretro são **singleton por processo** — o FBNeo guarda o estado da
máquina em variáveis globais. Por isso o host recusa carregar um segundo core no
mesmo processo.

### ROM / romset

O conteúdo dos chips da placa original. Um jogo de arcade não é um arquivo, é um
**conjunto** deles — o Last Blade 2 são 14 arquivos, o SFA3 são 21.

O FBNeo valida cada um por **CRC32**. Um arquivo faltando ou com CRC errado e o
jogo não roda.

**Nada relacionado a ROM está neste repositório**, e nenhum passo do laboratório
copia ROM para dentro da árvore de fontes.

### BIOS

Firmware da placa, separado do jogo. O Neo Geo tem um: o `neogeo.zip`, que
contém a rotina de boot, o BIOS do Z80, os tiles de texto e a tabela de zoom.

Sem ele, um zip de jogo perfeito não roda. E, para este laboratório, o BIOS é
**metade do código que executa** — por isso ele entra no hash comparado no
handshake, junto com a ROM.

### NVRAM / memory card

Memória que sobrevive ao desligamento. No Neo Geo guarda configurações e —
crucialmente — **créditos**.

Por que isso importa aqui: o FBNeo grava `<system>/fbneo/<jogo>.fs` ao
descarregar. Um peer que já rodou antes inicia com créditos, chega à tela de
título num frame diferente, e o script de boot aperta Start no momento errado.
Resultado: dois peers em menus diferentes, ou seja, desync. O laboratório apaga
esses arquivos antes de toda sessão.

### Script de boot

A sequência temporizada de botões que leva a máquina do reset até uma partida:
espera os logos, põe ficha, aperta Start, escolhe personagem, confirma.

É puramente **temporal** — "segure este botão por N frames, espere M" — e nunca
lê a memória do jogo. Ler RAM exigiria offsets específicos de revisão de ROM, e
tornaria o laboratório dependente de conhecer as entranhas do jogo, o que
derrotaria a demonstração de que rollback funciona numa simulação **opaca**.

Isso só é possível porque o emulador é determinístico: o mesmo script, do mesmo
reset, cai sempre na mesma tela no mesmo frame.

---

## Determinismo

### Determinismo

A propriedade de que a mesma entrada produz **sempre** exatamente a mesma saída.
Em toda máquina, em toda execução, em debug e em release.

É o alicerce de tudo: rollback é re-simulação, e re-simulação só converge se a
simulação for função apenas das suas entradas.

### Determinismo entre processos vs. segurança para rollback

São duas propriedades diferentes, e este laboratório precisou descobrir isso na
prática:

1. **Determinismo entre processos** — dois processos, mesma ROM, mesmos inputs,
   mesmo estado. Verificado por `just check-determinism`.
2. **Segurança para rollback** — `retro_unserialize` restaura *tudo* que
   `retro_run` vai voltar a ler. Verificado por `just check-rollback-safety`.

Um core pode ter a primeira e não a segunda: reprodutível a partir de um boot
frio, e ainda assim guardando estado fora do savestate. Foi exatamente o caso
aqui.

### Ponto fixo (fixed point)

Representar frações usando inteiros. A arena usa **Q23.8**: um `i32` onde os 8
bits de baixo são a parte fracionária, ou seja, a unidade é 1/256 de pixel.

Existe porque **ponto flutuante não é confiável entre máquinas**: o compilador
pode fundir `a*b+c` em FMA e mudar o arredondamento, o x87 tem precisão excedente
em registradores, `-ffast-math` permite reassociação, e bibliotecas matemáticas
diferem entre plataformas em funções transcendentais. Uma diferença de 1 ULP num
peer é um desync.

### ULP

*Unit in the Last Place* — a menor diferença representável entre dois números de
ponto flutuante vizinhos. A menor discordância possível, e o suficiente para
dessincronizar duas simulações.

### `overflow-checks`

Em Rust, `debug` entra em pânico ao estourar um inteiro; `release`, por padrão,
faz aritmética *wrapping*. Ou seja, o mesmo código produziria valores
**diferentes** nos dois perfis.

Este projeto liga a checagem também em release. Assim um peer compilado em debug
e outro em release se comportam igual, e um overflow vira falha ruidosa em vez de
estado silenciosamente errado.

### ASLR

*Address Space Layout Randomization* — o sistema operacional coloca a memória do
programa em endereços diferentes a cada execução. Nada na simulação pode depender
de um endereço, porque ele muda entre peers e entre execuções.

---

## Protocolo e segurança

### Handshake

A troca inicial em que os dois peers verificam que são compatíveis, **antes** de
a partida começar. Compara, nesta ordem: versão do protocolo, simulação, commit
da aplicação, hash da configuração, semente, hash do core, hash da ROM+BIOS, e
qual slot de jogador cada um quer.

A ordem importa: a primeira diferença encontrada é a que vira mensagem de erro,
então o motivo relatado é o mais fundamental.

É uma checagem de **compatibilidade**, não de segurança. Ela impede um peer
incompatível de gerar um desync confuso vinte segundos depois.

### HMAC

*Hash-based Message Authentication Code* — um carimbo criptográfico que prova que
uma mensagem veio de quem tem a chave e não foi alterada no caminho.

Aqui é HMAC-SHA256, 32 bytes, sobre **todo** datagrama. Um datagrama que falha o
HMAC é descartado antes de virar mensagem — não é recusado, é ignorado. O
sintoma de chave errada é, portanto, **silêncio**, não erro.

### Comparação em tempo constante

Comparar dois HMACs byte a byte com saída antecipada vaza informação: quanto
tempo a comparação levou diz quantos bytes bateram. A verificação usa comparação
de tempo constante para não vazar isso.

### Chave de sessão

O segredo compartilhado que alimenta o HMAC. Efêmera: gerada por execução,
guardada em SSM SecureString, nunca no estado do Terraform, nunca em linha de
comando (argumentos são visíveis no `ps` para qualquer usuário da máquina).

### `PeerTimeout`

O peer parou de mandar datagramas autenticados por mais de **3 segundos**. A
sessão encerra em vez de esperar para sempre.

Sutileza que virou bug: o peer *travado* é justamente o que mais precisa notar o
silêncio, e a checagem estava depois do caminho de saída rápida do stall. Um peer
ficou vivo acumulando 20 735 stalls num frame que nunca ia chegar.

---

## Infraestrutura

### Prometheus

Banco de dados de séries temporais que coleta métricas periodicamente. Cada peer
expõe suas métricas em `127.0.0.1:9898/metrics`, em texto, e o Prometheus
raspa isso.

Loopback de propósito: as métricas não vão para a rede.

### Grafana

A interface que desenha os gráficos por cima do Prometheus.

### JSONL

*JSON Lines*: um objeto JSON por linha. É o formato do log de sessão, escolhido
porque pode ser escrito incrementalmente (uma sessão que morre no meio deixa um
arquivo utilizável) e lido com `jq`.

### Terraform

Descreve a infraestrutura em arquivos versionados em vez de cliques no console.
`terraform apply` cria; `terraform destroy` remove.

Aqui descreve a VPC, a instância EC2 em Frankfurt, o grupo de segurança que abre
UDP/7000 **apenas para um /32**, e o bucket S3 temporário.

### SSM

*AWS Systems Manager*. Duas funções aqui: **Parameter Store** guarda a chave de
sessão criptografada, e **Session Manager** dá shell na instância sem abrir SSH
para a Internet.
