# 13 — Cobertura: o que foi validado e o que não foi

Este documento existe para que ninguém — inclusive quem escreveu — confunda "o
laboratório roda" com "o laboratório foi provado". Ele lista, sem generosidade,
o ambiente exato de cada medição feita até agora e o que continua sem evidência.

Mantido atualizado a cada rodada de experimentos.

---

## Os dois ambientes de medição

### A. Bancada local — uma máquina, dois processos, `127.0.0.1`

| | |
|---|---|
| Topologia | dois `rollback-bot` no mesmo host; P2 escuta em `127.0.0.1:7100`, P1 disca |
| Rede real | loopback — latência e perda desprezíveis |
| Degradação | **sintética**, injetada nos datagramas de saída de cada peer |
| Binário | o mesmo executável nos dois lados |
| CPU / SO / compilador | idênticos nos dois lados, por construção |

Os cinco perfis de rede de [08 — Experimentos](08-experimentos.md) foram medidos
assim.

### B. Sessão real — Madri ↔ Frankfurt

| | P1 | P2 |
|---|---|---|
| Onde | Madri, Espanha | Frankfurt, `eu-central-1` |
| Máquina | Arch Linux, Intel Core i7-10750H | Ubuntu 24.04, EC2 `t3.small` |
| Rede | Internet, sem degradação sintética | idem |

Duas sessões: The Last Blade 2 por 300 s e a arena por 150 s. Resultados
completos em [08 — Experimentos](08-experimentos.md#a-sessão-real-madri--frankfurt).

---

## O que isso valida de verdade

Esta parte é sólida, e não é pouca coisa — foi nela que quatro bugs reais
apareceram.

- **O motor de rollback.** Previsão, re-simulação, limite de previsão, buffer de
  estados, stalls. Exercitado por 20 minutos de luta emulada e por replays de
  100 000 frames na arena.
- **O protocolo, ponta a ponta.** Sockets UDP reais, datagramas reais, HMAC real,
  handshake real. Não é mock.
- **Comportamento sob perda, atraso, jitter e reordenação** — dentro dos limites
  do modelo sintético (ver adiante).
- **Detecção de desync.** 2 389 comparações de checksum concordando em cinco
  perfis, depois de o detector ser consertado.
- **Determinismo do emulador entre processos.** `just check-determinism` roda o
  core em dois processos separados, em segundos de relógio diferentes.
- **Segurança do savestate para rollback.** `just check-rollback-safety` prova
  que uma re-simulação de 300 frames não altera nada que o jogo consiga observar.
- **Determinismo entre máquinas diferentes.** 449 comparações de checksum
  concordando entre um i7-10750H rodando Arch e uma EC2 Ubuntu 24.04, nas duas
  simulações, com zero desyncs.
- **Sessão pela Internet real**, com a latência, a estabilidade e a perda que o
  caminho Madri–Frankfurt de fato tem.

---

## O que **não** foi validado

Em ordem de importância.

### ~~1. Determinismo entre máquinas diferentes~~ — FECHADO

Era o buraco mais sério: ponto fixo Q23.8, proibição de `HashMap`, FNV-1a
próprio, `overflow-checks` em release e nenhum valor derivado de endereço existem
todos para dois hosts diferentes concordarem bit a bit — e dois processos do
mesmo binário na mesma CPU teriam concordado mesmo se todas essas regras
estivessem erradas.

Fechado pela sessão Madri ↔ Frankfurt: **449 comparações de checksum
concordando**, entre CPUs, sistemas e libc diferentes, nas duas simulações. A
arena conta em separado, porque é o código que nós escrevemos.

### ~~2. Nenhuma sessão entre localizações diferentes~~ — FECHADO

Duas sessões executadas, coletadas e destruídas. O que se aprendeu, além do
óbvio, está em
[08 — Experimentos](08-experimentos.md#o-que-isso-prova-e-que-loopback-não-provava);
o resumo é que **os perfis sintéticos eram pessimistas em todas as dimensões** e
que parte do custo medido em loopback era da bancada, não do rollback.

Continuam sem medição, porque uma rodada de cinco minutos num link bom não os
produz:

- perda em rajada (o link real perdeu **zero** de 18 602 datagramas)
- rotas que mudam no meio da sessão
- congestionamento em horário de pico
- NAT doméstico dos dois lados — aqui um dos lados era uma EC2 com IP público

### 3. A rede sintética não é a Internet

O emulador de rede deste laboratório é deliberadamente simples, e isso tem
consequências:

| O modelo faz | A Internet faz |
|---|---|
| perda independente por datagrama (Bernoulli) | perda em **rajada** — vários seguidos, depois nenhum |
| jitter uniforme em ±N ms | distribuição com **cauda longa**, picos raros e grandes |
| atraso constante por perfil | atraso que muda com congestionamento e hora do dia |
| rota fixa | rotas que mudam no meio da sessão |

A rajada importa especialmente: a defesa deste protocolo contra perda é repetir
os últimos 8 inputs em cada datagrama, e **rajada é exatamente o pior caso para
uma janela de redundância**. Perder 8 datagramas seguidos derrota a redundância;
perder 8 espalhados não chega perto.

Isso deixou de ser teoria depois da sessão real. Medido, não estimado:

| | Madri↔Frankfurt real | `delay20` | `jitter30` | `loss2` |
|---|---|---|---|---|
| RTT | **50 ms** | 70 ms | 86 ms | 27 ms |
| Variação do RTT | **0,37 ms** | 0,5 ms | ~18 ms | 0,5 ms |
| Perda | **0,000%** (0 de 18 602) | 0% | 0% | 1,88% |

Os perfis são pessimistas em todas as dimensões — o lado certo de errar, mas vale
saber que `jitter30` descreve um Wi-Fi ruim e não um link entre datacenters.

O que continua **não** medido é a forma da perda: este link não perdeu nada, então
a hipótese de que rajadas derrotam a redundância de oito inputs segue sem teste.

### 4. O cliente gráfico com um humano

Todas as sessões foram bot contra bot. O `rollback-client` (SDL2, teclado e
gamepad, overlay) compila e é exercitado por testes, mas nenhuma partida com uma
pessoa no P1 foi jogada nesta bancada.

Isso deixa sem avaliação justamente a pergunta que o rollback existe para
responder: **como é jogar assim?** Nenhuma métrica deste repositório mede
percepção.

### 5. SFA3

Bloqueado por ROM incompleta: o set disponível não tem `sfa3.key`, a chave de
descriptografia CPS-2, e nenhuma das onze variantes do FBNeo dispensa uma. Ver
[09 — Jogos reais](09-sfa3.md).

O caminho libretro está validado — só que com The Last Blade 2 no lugar.

### 6. Uma execução por perfil

Não há intervalo de confiança em nenhum número deste repositório. Cada célula das
tabelas é uma amostra. Para ter dispersão, varie a semente:

```bash
for s in 1 2 3 4 5; do SEED=$s DURATION=60 ./ops/scripts/bench.sh; done
```

---

## Próximos passos, por valor

1. **Um humano no P1.** É o único buraco que nenhuma métrica deste repositório
   consegue fechar, e é a pergunta que o rollback existe para responder.
2. **Uma sessão longa em link ruim** — móvel, ou entre continentes — para ver
   perda em rajada, que é o único caso onde a redundância de oito inputs pode
   genuinamente falhar.
3. **Dispersão.** Variar a semente e agregar, para ter intervalos de confiança
   em vez de amostras únicas.
4. **SFA3**, se aparecer um set com `sfa3.key`.
