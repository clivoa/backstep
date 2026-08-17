# 01 — Teoria: por que rollback existe

## O problema

Um jogo de luta a 60 Hz decide o resultado de uma troca de golpes em janelas de
2 ou 3 frames — 33 a 50 milissegundos. Madrid–Frankfurt tem um RTT real de
30–45 ms. O tempo que a informação leva para atravessar o cabo é da mesma ordem
de grandeza que a decisão que o jogo precisa tomar.

Isso deixa o projetista com um problema que não tem solução limpa: **no frame
_N_, a máquina local não sabe o que o oponente apertou no frame _N_.** Não vai
saber tão cedo. Qualquer arquitetura de netcode é uma resposta a essa frase.

## As três respostas possíveis

### 1. Lockstep: esperar

Simule o frame _N_ só quando os dois inputs do frame _N_ tiverem chegado.

- **Vantagem:** simplicidade absoluta e correção trivial. Nunca há divergência.
- **Custo:** o jogo inteiro roda com a latência da rede embutida. Cada frame
  espera meio RTT. Com 40 ms de RTT, todo botão responde 2–3 frames depois, e
  qualquer variação de latência (jitter) vira travamento visível.

É o netcode "delay-based" clássico. Funciona bem em LAN e é insuportável entre
continentes.

### 2. Input delay: esperar de forma programada

Atrase deliberadamente o próprio input em _k_ frames, para que ele chegue ao
oponente a tempo. Se _k_ × 16,7 ms ≥ RTT/2, ninguém nunca espera.

- **Vantagem:** o jogo nunca trava. A latência vira constante e previsível.
- **Custo:** você pagou a latência da rede com latência de *input*. O jogo fica
  permanentemente menos responsivo, inclusive nos momentos em que a rede estava
  ótima. E é assimétrico com a percepção humana: um jogador sente 3 frames de
  delay muito mais do que sente 3 frames de correção visual.

### 3. Rollback: adivinhar e corrigir

Simule o frame _N_ **agora**, chutando o input do oponente. Quando o input real
chegar, se o chute estava certo, não faça nada. Se estava errado, volte ao
último estado correto e re-simule tudo desde ali com o input verdadeiro.

- **Vantagem:** o input local responde no frame em que foi apertado. Sempre.
  A latência da rede vira um problema *visual* (o oponente às vezes "corrige" de
  posição) em vez de um problema de *controle*.
- **Custo:** três coisas caras, detalhadas abaixo.

Rollback venceu porque troca a moeda certa. Um jogador de luta detecta latência
de input com uma precisão brutal, e tolera muito bem que o oponente ande alguns
pixels para o lado — porque durante a maior parte do tempo, o chute está certo.

## Por que o chute funciona

A previsão deste laboratório é a mais simples possível: **assuma que o oponente
continua fazendo o que estava fazendo** (`predict_remote`, em
`crates/rollback-core/src/session.rs`).

Isso parece ingênuo até você olhar a estatística real de um jogo de luta. Inputs
são segurados por muitos frames: direções ficam pressionadas durante caminhadas
inteiras, botões de carga por dezenas de frames, e há longos períodos de neutro.
Os experimentos deste repositório medem a acurácia dessa previsão em condições
adversas — veja [08 — Experimentos](08-experimentos.md).

O importante: **a previsão não precisa estar sempre certa.** Ela precisa estar
certa com frequência suficiente para que as correções sejam raras e rasas.

## Os três custos do rollback

### Custo 1: o estado precisa ser salvo e restaurado

A cada frame o motor salva um snapshot completo da simulação. Para voltar 6
frames, ele carrega o snapshot de 6 frames atrás e re-simula 6 frames.

Isso impõe um requisito duro à simulação: **todo o estado precisa ser
serializável e restaurável exatamente.** Não pode haver nada de relevante em
variáveis fora do snapshot — nem um contador de animação, nem um gerador de
números aleatórios, nem uma flag de "já toquei esse som".

Na arena, o snapshot tem 204 bytes. No SFA3 sob FBNeo, tem alguns megabytes —
e ainda assim cabe no orçamento de 16,7 ms por frame, porque `retro_serialize`
é essencialmente um `memcpy` de regiões contíguas.

### Custo 2: CPU

Um rollback de profundidade 6 significa simular 7 frames no tempo de 1. O
orçamento de frame precisa ter folga suficiente para o pior caso. É por isso que
existe um **limite de previsão** (8 frames neste laboratório): sem ele, uma
desconexão momentânea faria a máquina tentar re-simular centenas de frames de
uma vez.

O relatório mede isso diretamente como `resimulation_overhead`: frames
re-simulados por frame apresentado.

### Custo 3: determinismo obrigatório

Esta é a parte que mata projetos.

Se as duas máquinas, partindo do mesmo estado e recebendo os mesmos inputs,
produzirem estados diferentes — ainda que por um único bit — as duas simulações
divergiram e **tudo depois disso é ficção**. Os dois jogadores estão vendo jogos
diferentes.

Isso é chamado de *desync*, e a única defesa honesta é detectá-lo cedo e parar.
Este laboratório compara checksums de estado a cada 60 frames confirmados e
encerra a sessão imediatamente ao primeiro desacordo. As regras que evitam chegar
lá estão em [05 — Determinismo](05-determinismo.md).

## O que é "frame confirmado"

Vale fixar o vocabulário, porque quase toda a lógica gira em torno dele.

- **Frame atual** (`current_frame`): o próximo a ser simulado.
- **Frame confirmado** (`confirmed_frame`): o maior frame para o qual os inputs
  dos **dois** jogadores já são conhecidos. Nada antes dele pode mudar.
- **Profundidade de previsão** (`prediction_depth`): quantos frames à frente do
  confirmado a simulação já avançou especulativamente.

O rollback só pode alcançar até `prediction_depth` frames para trás. Por isso o
buffer de estados precisa ser **estritamente maior** que o limite de previsão —
16 estados para um limite de 8. Essa relação é validada em `SessionConfig::validate`
e é a razão de o teste `rolling_back_past_the_state_buffer_is_reported_not_silently_wrong`
existir.

## O que este laboratório acrescenta à teoria

Rollback funciona. Isso não está em disputa desde o GGPO. O que este laboratório
tenta fazer é diferente: **tornar o mecanismo visível e medível**.

- A arena existe para que se possa abrir o estado, contar os bytes e ver
  exatamente qual campo divergiu.
- O overlay do cliente pinta cada frame por como ele foi produzido: confirmado,
  previsto, corrigido ou travado.
- O emulador de rede injeta atraso, jitter, perda, duplicação e reordenação com
  semente fixa, para que um experimento seja *repetível*.
- O SFA3 existe para provar que nada disso depende de a simulação ser de
  brinquedo: o mesmo motor dirige um emulador de arcade opaco de vários
  megabytes.

## Leitura adicional

- GGPO (Tony Cannon) — a implementação que estabeleceu o padrão.
- "Fight the Lag!" — a explicação clássica de rollback do próprio autor do GGPO.
- Fightcade — a rede onde isso roda na prática, com FBNeo, há anos. Aqui é
  referência comparativa, não dependência.
