# 08 — Experimentos

## Pergunta

O que acontece com um jogo de luta a 60 Hz sob rollback quando a rede piora?
Especificamente: quanto trabalho extra de CPU custa, com que frequência o jogador
vê uma correção, e em que ponto a simulação começa a travar.

## Método

Cinco perfis de rede, 180 segundos cada, bot contra bot na arena, semente fixa
(4242), input delay de 1 frame, limite de previsão de 8 frames, buffer de 16
estados.

| Perfil | Atraso | Jitter | Perda | Reordenação | RTT medido |
|---|---|---|---|---|---|
| `natural` | — | — | — | — | 16,6 ms |
| `delay20` | 20 ms | — | — | — | 70 ms |
| `jitter30` | 30 ms | ±15 ms | — | — | 84–88 ms |
| `loss2` | — | — | 2% | — | 27 ms |
| `combined` | 40 ms | ±20 ms | 2% | 0,5% | 97–105 ms |

**O que cada perfil imita, o que ele isola, e o que é jitter afinal:** cada um
tem explicação própria em
[00 — Glossário: os cinco perfis](00-glossario.md#os-cinco-perfis-de-rede).

Em resumo: `natural` é o controle, `delay20` isola a distância, `jitter30` isola
a *variação* da latência, `loss2` isola a perda para testar a redundância, e
`combined` é o pior caso que o laboratório se propõe a aguentar.

O impedimento é aplicado aos datagramas de **saída** de cada peer, então o RTT
observado é aproximadamente o dobro do atraso unidirecional configurado. Isso é
deliberado e está explicado em [03 — Protocolo](03-protocolo.md).

Reprodução:

```bash
just bench                  # os cinco perfis, 180 s cada, ~15 min
just bench 30      # versão rápida
```

Os dois peers são bots com semente fixa e o emulador de rede também é semeado, de
modo que a única coisa que varia entre execuções é a rede real por baixo — que,
numa bancada local, é praticamente nada.

## Como ler os números

- **Frames apresentados diferem entre os peers** por construção: cada lado desenha
  o próprio presente. O que precisa bater são os checksums dos frames confirmados.
- **Rollbacks são assimétricos.** Quem está alguns milissegundos atrás corrige
  mais, porque suas previsões cobrem uma janela maior. Números diferentes entre
  P1 e P2 são esperados; o que não pode diferir é o estado confirmado.
- **A perda é inferida** a partir de lacunas na sequência. Um datagrama atrasado
  aparece como perda até chegar.
- **Não há latência unidirecional.** Só RTT.

## Resultados

### Execução de referência

Arena, 180 s por perfil, semente 4242, dois bots em loopback, commit
`ee5ca9422d88`. Os dez logs completos estão em `artifacts/report/summary.csv`.

| Perfil | Peer | FPS | Rollbacks | Prof. média | Prof. máx | Acurácia | Trabalho extra | Stalls | Checksums | Desync | RTT | Var. RTT | Perda | Bitrate | CPU |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| natural | P1 | 60,01 | 1 | 1,00 | 1 | — | 0,01% | 0 | 179 | não | 16,7 ms | 0,5 ms | 0,00% | 34,9 kbit/s | 2,0 s |
| natural | P2 | 60,01 | 1 | 1,00 | 1 | — | 0,01% | 0 | 180 | não | 16,7 ms | 0,4 ms | 0,00% | 34,9 kbit/s | 2,2 s |
| delay20 | P1 | 60,01 | 1 | 1,00 | 1 | — | 0,01% | 0 | 179 | não | 77,4 ms | 11,2 ms | 0,00% | 34,9 kbit/s | 2,2 s |
| delay20 | P2 | 60,01 | **727** | 4,00 | 5 | 93,3% | **26,9%** | 0 | 180 | não | 83,4 ms | 0,5 ms | 0,00% | 34,9 kbit/s | 2,2 s |
| jitter30 | P1 | 60,01 | 1 | 1,00 | 1 | — | 0,01% | 0 | 179 | não | 84,0 ms | 19,2 ms | 0,00% | 34,9 kbit/s | 2,1 s |
| jitter30 | P2 | 60,01 | **683** | 4,25 | 5 | 93,7% | **26,9%** | 0 | 180 | não | 86,9 ms | 17,9 ms | 0,00% | 34,9 kbit/s | 2,1 s |
| loss2 | P1 | 60,01 | 1 | 1,00 | 1 | 87,5% | 0,01% | 0 | 176 | não | 16,5 ms | 0,5 ms | 1,89% | 34,9 kbit/s | 2,0 s |
| loss2 | P2 | 60,01 | 12 | 1,00 | 1 | 94,2% | 0,11% | 0 | 177 | não | 16,7 ms | 0,4 ms | 1,89% | 34,9 kbit/s | 2,1 s |
| combined | P1 | 60,01 | 122 | 1,00 | 1 | 93,3% | 1,1% | 0 | 177 | não | 99,3 ms | 19,5 ms | 1,95% | 34,9 kbit/s | 2,0 s |
| combined | P2 | 60,01 | **700** | 4,83 | **6** | 93,5% | **31,3%** | 1 | 178 | não | 102,7 ms | 19,8 ms | 1,95% | 34,9 kbit/s | 2,0 s |

Zero desyncs em 1 800 segundos de sessão e 1 786 comparações de checksum.

## O mesmo motor, num emulador de verdade

A arena mede o motor de rollback. Ela não mede o que acontece quando a simulação
é opaca e o estado é grande. Para isso, os mesmos cinco perfis rodaram com **The
Last Blade 2** sob o FBNeo — mesmo `RollbackSession`, mesmo protocolo, mesmo
runner; só a implementação de `Simulation` muda.

```bash
just e2e 90 lastblade2 /caminho/lastbld2.zip
```

Noventa segundos por perfil em vez de 180, porque o script de boot consome os
primeiros ~33 segundos levando a máquina pelos menus (ver [09](09-sfa3.md)).

### A diferença que importa: o tamanho do estado

| | arena | Last Blade 2 |
|---|---|---|
| `state_bytes` | 204 | **415 155** |
| razão | 1× | **2 036×** |
| CPU por 90 s de sessão | ~1 s | **~34 s** |
| fração de um núcleo | ~1% | **~38%** |

Esse é o número que a arena não conseguia mostrar. Salvar estado na arena é
copiar 204 bytes; no emulador é `retro_serialize` de 405 KB, e o rollback faz
isso **uma vez por frame** mais uma vez por frame re-simulado.

`LibretroSimulation` guarda o checksum do último snapshot em um `Cell`
justamente por isso: sem esse cache, o par `save_state` + `checksum` que a
sessão faz a cada frame custaria dois `retro_serialize` em vez de um. A 60 Hz com
405 KB, isso é a diferença entre caber e não caber no orçamento de 16,7 ms.

### Execução de referência

240 s por perfil, semente 4242, dois bots jogando o repertório completo
(combos encadeados, motion inputs, defesa alta e baixa, repel, agarrão), luta de
verdade com rounds completos.

| Perfil | Peer | FPS | Rollbacks | Prof. média | Prof. máx | Acurácia | Trabalho extra | Stalls | Checksums | Desync | RTT | Perda | CPU |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| natural | P1 | 60,01 | 0 | 0,00 | 0 | — | 0,0% | 0 | 240 | não | 16,6 ms | 0,00% | 85,6 s |
| natural | P2 | 60,01 | 0 | 0,00 | 0 | — | 0,0% | 0 | 240 | não | 15,7 ms | 0,00% | 85,1 s |
| delay20 | P1 | 60,00 | 0 | 0,00 | 0 | — | 0,0% | 0 | 240 | não | 69,9 ms | 0,00% | 69,3 s |
| delay20 | P2 | 59,85 | **1006** | 6,53 | 7 | 93,0% | **45,7%** | 37 | 240 | não | 70,1 ms | 0,00% | 88,8 s |
| jitter30 | P1 | 60,00 | 0 | 0,00 | 0 | — | 0,0% | 0 | 240 | não | 88,5 ms | 0,00% | 73,4 s |
| jitter30 | P2 | 59,85 | **1006** | 6,90 | **8** | 93,0% | **48,2%** | 37 | 240 | não | 84,2 ms | 0,00% | 93,7 s |
| loss2 | P1 | 60,01 | 19 | 1,00 | 1 | 93,0% | 0,1% | 0 | 237 | não | 28,2 ms | 1,88% | 88,0 s |
| loss2 | P2 | 60,01 | 4 | 1,00 | 1 | 95,1% | 0,0% | 0 | 237 | não | 27,5 ms | 1,88% | 87,7 s |
| combined | P1 | 60,00 | 125 | 1,11 | 2 | 93,1% | 1,0% | 0 | 239 | não | 97,6 ms | 2,00% | 78,0 s |
| combined | P2 | 59,85 | **1004** | 4,84 | 7 | 93,0% | **33,7%** | 39 | 236 | não | 105,6 ms | 2,00% | 92,1 s |

**Vinte minutos de luta emulada sob rollback, 2 389 checksums comparados, zero
desyncs.**

### O que muda em relação à arena

**60 Hz aguentam até 48% de trabalho extra.** No `jitter30` o P2 re-simulou quase
metade de um frame a mais por frame — em cima de um estado de 405 KB — e ainda
entregou 59,85 fps. O custo aparece na CPU (93,7 s de CPU para 240 s de sessão,
~39% de um núcleo) e não na taxa de quadros.

**A profundidade encosta no limite.** Na arena a profundidade máxima foi 6 com
limite 8. Aqui foi **8**, com média 6,9 e 37 stalls. O estado maior torna
`save_state`/`load_state` mais caros, o peer da frente ganha mais fase, e a
janela de previsão enche. É o dimensionamento sendo exercido de verdade, não com
folga.

**A acurácia de previsão não mudou: ~93,0%.** Este é o resultado que mais vale a
pena olhar. A arena, com bots simplórios, deu 93,5%. Um jogo de luta real, com
um bot que faz combos encadeados de três golpes, meia-lua com quatro direções em
doze frames e guarda segurada por quase um segundo, dá 93,0%.

A regra "repita o último input confirmado" não é esperta — ela funciona porque
inputs de jogo de luta são **segurados**, e isso vale igualmente para uma arena
de brinquedo e para o Last Blade 2. É a hipótese central do rollback, medida
duas vezes em simulações que não têm nada em comum além do gênero.

**A perda continua quase de graça.** `loss2` produziu 19 e 4 rollbacks em 240 s,
contra 1 006 do `delay20`. A redundância de oito inputs entrega o input perdido
no datagrama seguinte, muito antes de ele ser necessário. Perda e latência não
são o mesmo problema, e o rollback só é sensível ao segundo — igualzinho à
arena, agora com um emulador no meio.

## Interpretação

### 60 Hz sustentados em toda condição

`effective_fps` é 60,01 nos dez logs, inclusive no perfil combinado onde um peer
re-simulou 31% de trabalho extra. A um estado de 204 bytes o rollback custa
CPU desprezível — 2 segundos de CPU para 180 segundos de sessão, ou cerca de 1%
de um núcleo. Esse número não se transfere para o SFA3, onde o `retro_serialize`
domina; é justamente por isso que ele é medido.

### A assimetria é o resultado mais interessante

Sob `delay20`, o P2 fez 727 rollbacks e o P1 fez **um**.

Isso não é bug: é como o rollback funciona. Os dois peers rodam relógios de
frame independentes, então há uma diferença de fase fixa entre eles. Quem está
*à frente* chega no frame N antes de o input do peer para o frame N existir, e
portanto prevê e corrige. Quem está *atrás* recebe o input antes de precisar
dele e nunca chuta.

Nesta bancada o P2 hospeda e completa o handshake um round-trip antes do P1, então
ele fica permanentemente à frente. Numa sessão real Madrid–Frankfurt a fase é
arbitrária, mas igualmente persistente: **um dos dois jogadores paga
essencialmente todo o custo de rollback da partida**, e qual deles é decidido por
alguns milissegundos na largada.

Isso tem uma consequência prática que os números tornam concreta: comparar
"quantos rollbacks meu jogo faz" entre dois clientes não diz nada sobre a
qualidade da rede, e sim sobre quem começou primeiro.

O sinal de que os dois estão de fato jogando o mesmo jogo não é a simetria dos
rollbacks — é `checksums_compared` subir nos dois lados com `desync = false`.
Isso vale nas dez sessões.

### A redundância de oito inputs absorve 2% de perda quase sem custo

`loss2` mede 1,89% de perda inferida — a impedância pedida — e produz **12
rollbacks em 180 segundos** no peer que está à frente, contra 727 do `delay20`.

A perda quase não vira rollback porque o input perdido chega no datagrama
seguinte, 16,7 ms depois, muito antes de ser necessário. É exatamente o que a
repetição de oito inputs foi feita para fazer, e é a razão de não haver
retransmissão no protocolo.

Compare com `combined`, que soma perda ao atraso: os 700 rollbacks vêm do atraso,
não da perda. Perda e latência não são o mesmo problema, e o rollback só é
sensível ao segundo.

### Jitter não é pior que atraso, para o rollback

`jitter30` (30 ± 15 ms) e `delay20` (20 ms fixos) produzem praticamente o mesmo
resultado: ~700 rollbacks, profundidade média ~4, acurácia ~93%. A variação do
RTT sobe de 0,5 ms para 17,9 ms, como esperado, mas a profundidade máxima do
rollback mal se move (5 nos dois).

Faz sentido: o rollback já corrige a cada frame. Um datagrama que chega 15 ms
tarde demais é corrigido pelo mesmo mecanismo que corrige um que chega 20 ms
tarde. É o *lockstep* que sofre com jitter, porque ele precisa esperar o pior
caso; o rollback só precisa que o pior caso caiba na janela de previsão.

### A profundidade fica bem abaixo do limite

O máximo observado foi **6**, contra um limite de 8 e um buffer de 16 estados.
Um único stall apareceu nas dez sessões (P2 sob `combined`).

Ou seja: o dimensionamento tem folga real para essas condições. Um limite de
previsão menor — 6, por exemplo — encostaria com frequência sob `combined`, e um
maior não compraria nada, só aumentaria o pior caso de CPU por correção.

### Acurácia de previsão de ~93,5%, estável

Sob qualquer perfil com atraso, a regra "repita o último input confirmado"
acerta cerca de 93,5% dos frames que precisou chutar. É notavelmente insensível
ao perfil: 93,3% em `delay20`, 93,7% em `jitter30`, 93,5% em `combined`.

Isso reforça o ponto de [01 — Teoria](01-teoria.md): a previsão não funciona por
ser esperta, e sim porque inputs de jogo de luta são segurados por muitos frames.
E como estes são bots, que mudam de input com mais frequência que humanos, é
razoável ler 93,5% como um **piso**.

### O bitrate não depende de nada

34,9 kbit/s em todos os dez logs, com variação de 0,03%. Só inputs trafegam, a
uma taxa fixa de 60 Hz, com oito repetições de largura fixa. Nem perda, nem
atraso, nem rollback mudam quanto o protocolo põe no cabo.

## A sessão real: Madri ↔ Frankfurt

Tudo acima é loopback com degradação sintética. Esta seção é a Internet.

| | P1 | P2 |
|---|---|---|
| Onde | Madri, Espanha | Frankfurt, `eu-central-1` |
| Máquina | Arch Linux, Intel Core i7-10750H | Ubuntu 24.04, EC2 `t3.small` |
| Papel | disca | escuta em UDP/7000 |

**Perfil `natural` — sem degradação sintética.** Injetar atraso por cima de um
link real só embaralharia a medição; o objetivo aqui é medir o que existe.

### The Last Blade 2, 300 segundos

| Métrica | P1 (Madri) | P2 (Frankfurt) |
|---|---|---|
| `effective_fps` | 60,01 | 60,01 |
| Rollbacks | **1 280** | 31 |
| Profundidade média | 2,04 | 1,03 |
| Profundidade máxima | 4 | 2 |
| Acurácia de previsão | 92,9% | 92,4% |
| Trabalho extra | 14,5% | 0,2% |
| Stalls | **0** | **0** |
| Checksums comparados | **300** | **300** |
| Desync | **não** | **não** |
| SRTT | 49,9 ms | 51,6 ms |
| Variação do RTT | **0,37 ms** | 3,22 ms |
| Perda | **0,000%** | **0,000%** |
| CPU | 116 s | 46 s |

### A arena, 150 segundos

Rodada logo em seguida, no mesmo link, trocando só a simulação.

| Métrica | P1 (Madri) | P2 (Frankfurt) |
|---|---|---|
| Rollbacks | 19 | **601** |
| Profundidade média / máxima | 1,00 / 1 | 2,02 / 3 |
| Acurácia de previsão | 91,7% | 93,3% |
| Checksums comparados | 149 | 150 |
| Desync | **não** | **não** |
| SRTT | 51,8 ms | 50,0 ms |
| CPU | 1,95 s | 1,54 s |

### O que isso prova, e que loopback não provava

**1. Determinismo entre máquinas diferentes.** Este era o buraco mais sério do
projeto ([13 — Cobertura](13-cobertura.md)). 449 comparações de checksum
concordando, entre um desktop Arch Linux com um i7-10750H e uma EC2 Ubuntu 24.04
— CPU diferente, sistema diferente, libc diferente. Zero desyncs.

A arena importa em separado aqui: ela é o código que **nós** escrevemos, e as
regras de ponto fixo Q23.8, proibição de `HashMap`, FNV-1a próprio e
`overflow-checks` em release existem exatamente para este cenário. Dois processos
do mesmo binário na mesma CPU teriam concordado mesmo se todas estivessem
erradas. Agora não teriam.

**2. Os perfis sintéticos eram pessimistas — em todas as dimensões.**

| | Link real Madri↔Frankfurt | `delay20` | `jitter30` | `loss2` |
|---|---|---|---|---|
| RTT | **50 ms** | 70 ms | 86 ms | 27 ms |
| Variação do RTT | **0,37 ms** | 0,5 ms | ~18 ms | 0,5 ms |
| Perda | **0,000%** | 0% | 0% | 1,88% |

Zero datagramas perdidos em **18 602 enviados** ao longo de cinco minutos. E a
variação do RTT de 0,37 ms significa que fibra entre cidades europeias é **muito**
mais estável do que o perfil `jitter30` supõe.

Errar para o lado pessimista é o lado certo de errar — mas vale registrar que
`jitter30` e `loss2` descrevem um Wi-Fi ruim, não um link entre datacenters.

**3. A previsão volta a acertar ~93%.** Terceira medição independente: arena em
loopback 93,5%, Last Blade 2 em loopback 93,0%, Last Blade 2 na Internet real
**92,9%**. A hipótese central do rollback — inputs de luta são segurados — não
depende do jogo nem da rede.

**4. A profundidade real é muito menor que a simulada.** Com RTT comparável
(50 ms real vs 70 ms no `delay20`), a profundidade média caiu de **6,53 para
2,04**, a máxima de 7 para 4, e os stalls de **37 para zero**.

Duas razões, e a segunda é uma lição sobre o método:

- o link real tem variação de RTT ~50× menor;
- em loopback os **dois peers dividiam a mesma CPU**, e a disputa por
  escalonamento adiciona deriva de fase que não existe quando cada peer tem sua
  própria máquina.

Ou seja: parte do custo que os experimentos em loopback mediram era do
laboratório, não do rollback.

**5. A assimetria é real, e trocou de lado entre as duas rodadas.** No Last Blade
2 Madri pagou 1 280 rollbacks contra 31 de Frankfurt; na arena, quinze minutos
depois e no mesmo link, **Frankfurt pagou 601 contra 19 de Madri**.

Nada mudou na rede. Mudou quem completou o handshake primeiro. É a demonstração
mais limpa possível de que "quantos rollbacks meu cliente faz" não mede a
qualidade da conexão — mede a fase de largada.

**6. O custo de CPU se confirma fora da bancada.** 116 s de CPU para 300 s de
sessão em Madri (~39% de um núcleo, exatamente como em loopback), contra 46 s em
Frankfurt — que quase não fez rollback. O peer que paga a assimetria paga também
em CPU.

### Reprodução

```bash
just check-determinism /caminho/lastbld2.zip     # antes de gastar
just aws-up lastblade2 /caminho/lastbld2.zip
# noutro terminal, o peer local:
export ROLLBACK_SESSION_KEY=$(cat artifacts/session.key)
./target/release/rollback-bot --sim lastblade2 --player p1 \
  --peer "$(terraform -chdir=terraform output -raw peer_address)" --bind 0.0.0.0:0 \
  --profile natural --seed 4242 --duration 300 --mode play \
  --core cores/fbneo_libretro.so --rom /caminho/lastbld2.zip \
  --system-dir artifacts/system --log-dir artifacts/logs
just collect      # SEMPRE antes
just aws-down
```

Custo da rodada inteira (duas sessões, ~40 min de instância): abaixo de
US$ 0,05. Ver [10 — Custos](10-custos.md).

## Os números daqui vêm de sessões sem gravação

Vale deixar explícito porque é fácil confundir: os vídeos de
[14 — Vídeo](14-video.md) não são a fonte destes números.

Gravar custa ~65% de um núcleo por peer, e isso desloca a medição — no perfil
`natural` em loopback, o RTT p50 sobe de **16,6 ms sem gravação para 38,0 ms
com**. Não é a rede: é o laço de frame ficando mais lento numa máquina ocupada.

Os vídeos ilustram o comportamento com fidelidade. Os números vêm daqui.

## O que os experimentos não medem

> **Antes de tudo: uma máquina, dois processos, loopback.** Nenhum número deste
> documento veio de dois computadores diferentes, muito menos de localizações
> diferentes. Isso deixa **sem nenhuma evidência** justamente as regras de
> determinismo entre máquinas (ponto fixo, sem `HashMap`, `overflow-checks`) que
> boa parte do projeto existe para garantir. O inventário completo do que foi e
> do que não foi validado está em [13 — Cobertura](13-cobertura.md).

- **Percepção.** Nenhum número aqui diz se o jogo *parece* bom. Um rollback de
  profundidade 2 é invisível; um de profundidade 8 num momento de troca de golpes
  é bem perceptível, e a média não distingue os dois.
- **Bots não jogam como humanos.** Eles mudam de input com uma cadência
  regular. Um humano segura direções por muito mais tempo, o que torna a previsão
  mais fácil — então a acurácia medida aqui é provavelmente um **piso**.
- **Loopback não é a Internet.** Os perfis injetam atraso e perda estatísticos e
  independentes. Perda real na Internet é em rajadas, e rajadas são o pior caso
  para uma janela de redundância de 8 inputs.
- **Uma execução por perfil.** Não há intervalo de confiança. Para isso, varie a
  semente e agregue.

## Extensões naturais

```bash
# variar o input delay: quanto ele compra em acurácia?
for d in 0 1 2 3; do
  DURATION=60 SEED=4242 PROFILES=combined \
    ./ops/scripts/bench.sh --input-delay $d
done

# variar a semente, para ter dispersão
for s in 1 2 3 4 5; do SEED=$s DURATION=60 ./ops/scripts/bench.sh; done

# o mesmo, com SFA3
just bench 180 sfa3 /caminho/sfa3.zip
```

A pergunta mais interessante que este laboratório está preparado para responder e
ainda não respondeu: **qual input delay minimiza a soma de latência percebida e
correções visíveis, para um dado perfil de rede?** Todos os números necessários
já saem no `summary.csv`.
