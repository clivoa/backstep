# 14 — Vídeo: ver o rollback acontecendo

> Termos como *rollback*, *stall*, *profundidade* e *perfil* estão definidos em
> [00 — Glossário](00-glossario.md).

## O problema de documentar rollback

Rollback funciona quando você **não** percebe. A correção acontece dentro de um
frame de tela; o jogador vê um jogo contínuo. Gravar uma sessão e assistir prova
que o jogo rodou, e mais nada.

O que transforma a gravação em documentação é pôr a telemetria ao lado dela. O
log JSONL já registra cada frame apresentado, cada rollback com sua
profundidade, cada stall — então dá para queimar isso no vídeo, no frame exato
em que aconteceu.

O resultado é o ponto: **uma luta perfeitamente fluida enquanto o contador
marca mil correções passando.**

## Como funciona

```
rollback-bot --record sessao.mp4        frames apresentados -> ffmpeg -> H.264
        |
        +-- sessao.jsonl                eventos por frame
                |
                v
   annotate-video.py                    JSONL -> legenda ASS -> queima no vídeo
```

### Os frames vêm da simulação, não da tela

Um gravador de tela captura o que o compositor mostrou, na taxa em que ele
compôs. Isso é inútil aqui, porque a afirmação interessante é sobre **quais**
frames chegaram ao jogador.

O `--record` grava exatamente os frames produzidos em `OutputMode::Present`: um
por frame avançado, nenhum dos re-simulados. Então o arquivo é, frame a frame, o
que o jogador viu — e uma gravação de 60 Hz de uma sessão que sustentou 60 Hz é
prova de que ela sustentou.

Frames com geometria diferente da declarada são **descartados e contados**, não
escritos: um frame curto deslocaria todos os bytes seguintes e transformaria o
resto do vídeo em ruído.

### A telemetria fica numa faixa, não sobre o jogo

Desenhada por cima, colidia com o HUD do próprio jogo — barra de vida e
cronômetro moram exatamente no mesmo canto, e os dois ficavam ilegíveis. A faixa
custa um pouco de altura e mantém a saída do emulador intocada, o que também
significa que o vídeo continua mostrando o que o jogador de fato via.

O marcador `ROLLBACK -N` fica sobre a imagem, porque ali ele é informação
temporal, e some em 0,25 s. Stalls usam o estilo barulhento e duram o que
duraram.

## Gerar os vídeos

```bash
just record-scenarios /caminho/lastbld2.zip
just record-scenarios /caminho/lastbld2.zip 120 "natural combined"
```

Sai em `artifacts/video/`: para cada perfil, um vídeo por peer e um lado a lado.

Os **dois** peers são gravados de propósito. Sob qualquer perfil com atraso os
dois lados fazem quantidades absurdamente diferentes de trabalho, e um vídeo de
um peer só mostraria uma luta fluida e esconderia o fenômeno inteiro.

Para anotar uma gravação avulsa:

```bash
just annotate bruto.mp4 sessao.jsonl saida.mp4 "rótulo"
```

## O que cada vídeo mostra

Gravações de 90 s por perfil, bot contra bot em loopback, semente 4242.

| Vídeo | P1 | P2 | O que observar |
|---|---|---|---|
| `natural-both` | 0 rollbacks | 0 rollbacks | O controle. Nada acontece: os inputs chegam antes de serem necessários. |
| `delay20-both` | **0** | **260**, 40 stalls | A assimetria. Mesma luta, mesmo frame, e só um lado trabalha. |
| `jitter30-both` | 0 | 260, 39 stalls | Igual ao anterior a olho nu — jitter não é pior que atraso, para rollback. |
| `loss2-both` | 0 | **5** | Perda quase não vira rollback. A redundância de oito inputs entrega o input perdido antes de ele fazer falta. |
| `combined-both` | 65 | 259, 40 stalls | Os dois lados trabalhando, e os stalls visíveis. |
| `aws-madrid-frankfurt-both` | **544**, 133 stalls | 13 | O link real, Madri ↔ Frankfurt. |

### O frame que resume o projeto

Em qualquer vídeo lado a lado, pause em qualquer momento: **as duas metades
mostram a mesma imagem, pixel a pixel**, enquanto os contadores mostram números
completamente diferentes.

Isso é o laboratório inteiro numa imagem. As duas máquinas rodam o mesmo jogo. O
trabalho que cada uma faz para conseguir isso não tem nada a ver com o que a
outra faz.

## Uma ressalva metodológica importante

**Gravar custa CPU, e a CPU muda a medição.**

Cada `ffmpeg` consome ~65% de um núcleo codificando 60 fps. Numa gravação
loopback isso significa dois emuladores mais dois codificadores disputando a
mesma máquina, e o efeito aparece nos números:

| `natural`, loopback | RTT p50 |
|---|---|
| sessão normal | **16,6 ms** |
| sessão gravada | **38,0 ms** |

O RTT mais que dobrou, e não foi a rede — foi o laço de frame ficando mais lento
porque a máquina estava ocupada.

Consequência prática, e vale respeitá-la:

- **Os números de [08 — Experimentos](08-experimentos.md) vêm de sessões sem
  gravação.** São a medição.
- **Os vídeos são ilustração.** Mostram o comportamento com fidelidade —
  assimetria, stalls, a sincronia frame a frame — mas os contadores neles estão
  inflados pelo custo de gravar.

Nunca cite um número lido de um vídeo. Cite o `summary.csv` da sessão
equivalente sem gravação.

## Limitações

- **Só simulações emuladas.** A arena não tem framebuffer — ela é simulada, não
  desenhada, fora do cliente SDL. Gravar a arena exigiria um rasterizador
  próprio no bot, que ainda não existe.
- **Sem áudio.** O host descarta áudio durante a re-simulação, e sincronizar o
  que sobra com o vídeo é um problema em si. Os vídeos são mudos.
- **Sem pixel aspect.** A saída de arcade não é de pixel quadrado; o vídeo é
  escalado 3× com vizinho mais próximo e nada mais. Fica "esticado" em relação a
  um CRT, e isso é deliberado: corrigir exigiria escolher uma proporção que o
  core não informou.
