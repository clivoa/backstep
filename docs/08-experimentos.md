# 08 — Experimentos

## Pergunta

O que acontece com um jogo de luta a 60 Hz sob rollback quando a rede piora?
Especificamente: quanto trabalho extra de CPU custa, com que frequência o jogador
vê uma correção, e em que ponto a simulação começa a travar.

## Método

Cinco perfis de rede, 180 segundos cada, bot contra bot na arena, semente fixa
(4242), input delay de 1 frame, limite de previsão de 8 frames, buffer de 16
estados.

| Perfil | Atraso | Jitter | Perda | Reordenação |
|---|---|---|---|---|
| `natural` | — | — | — | — |
| `delay20` | 20 ms | — | — | — |
| `jitter30` | 30 ms | ±15 ms | — | — |
| `loss2` | — | — | 2% | — |
| `combined` | 40 ms | ±20 ms | 2% | 0,5% |

O impedimento é aplicado aos datagramas de **saída** de cada peer, então o RTT
observado é aproximadamente o dobro do atraso unidirecional configurado. Isso é
deliberado e está explicado em [03 — Protocolo](03-protocolo.md).

Reprodução:

```bash
just bench                  # os cinco perfis, 180 s cada, ~15 min
just bench duration=30      # versão rápida
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

<!-- RESULTADOS -->

## Interpretação

<!-- INTERPRETACAO -->

## O que os experimentos não medem

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
just bench sim=sfa3 rom=/caminho/sfa3.zip
```

A pergunta mais interessante que este laboratório está preparado para responder e
ainda não respondeu: **qual input delay minimiza a soma de latência percebida e
correções visíveis, para um dado perfil de rede?** Todos os números necessários
já saem no `summary.csv`.
