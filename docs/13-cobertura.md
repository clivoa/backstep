# 13 — Cobertura: o que foi validado e o que não foi

Este documento existe para que ninguém — inclusive quem escreveu — confunda "o
laboratório roda" com "o laboratório foi provado". Ele lista, sem generosidade,
o ambiente exato de cada medição feita até agora e o que continua sem evidência.

Mantido atualizado a cada rodada de experimentos.

---

## Ambiente de todas as medições até aqui

**Uma máquina. Dois processos. `127.0.0.1`.**

| | |
|---|---|
| Topologia | dois `rollback-bot` no mesmo host; P2 escuta em `127.0.0.1:7100`, P1 disca |
| Rede real | loopback — latência e perda desprezíveis |
| Degradação | **sintética**, injetada nos datagramas de saída de cada peer |
| Binário | o mesmo executável nos dois lados |
| CPU / SO / compilador | idênticos nos dois lados, por construção |

Tudo que aparece nas tabelas de [08 — Experimentos](08-experimentos.md) foi
medido assim.

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

---

## O que **não** foi validado

Em ordem de importância.

### 1. Determinismo entre máquinas diferentes — sem nenhuma evidência

Este é o buraco mais sério, porque é exatamente o que boa parte do projeto foi
desenhada para garantir:

- ponto fixo Q23.8 em vez de ponto flutuante
- proibição de `HashMap` na simulação
- FNV-1a próprio em vez de `DefaultHasher`
- `overflow-checks` ligado também em release
- nada derivado de endereço de memória

**Todas essas regras existem para dois hosts com CPUs, compiladores e sistemas
diferentes concordarem bit a bit.** Rodar dois processos do mesmo binário na
mesma CPU não testa nenhuma delas — o resultado seria idêntico mesmo se todas
estivessem erradas.

O teste mais barato e mais informativo do projeto inteiro é, portanto, **rodar os
dois peers em máquinas diferentes**, nem que seja na mesma sala.

### 2. Nenhuma sessão entre localizações diferentes

O Terraform que descreve o peer em Frankfurt está escrito, revisado e validado —
e **nunca foi aplicado**. Não existe `terraform.tfstate` nem `terraform.tfvars`
no repositório, o que é a prova de que `aws-up` jamais rodou.

Sem isso, seguem sem medição:

- latência **real** entre continentes, com a cauda que ela tem
- perda em rajada, que é como a Internet perde de verdade
- NAT, firewall, travessia UDP entre redes domésticas e nuvem
- rotas assimétricas (o caminho de ida diferente do de volta)
- MTU no caminho real
- relógios não sincronizados entre os peers

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

Calibração honesta: Madri–Frankfurt são ~1 900 km, algo em torno de 30–40 ms de
RTT real. Os perfis `delay20` (70 ms) e `jitter30` (86 ms) são portanto **mais
duros** que o link real em atraso — mas mais gentis em formato de perda.

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

## Próximo passo planejado

**Sessão real entre localizações diferentes**, usando a infraestrutura descrita
em [06 — AWS](06-aws.md): peer local ↔ EC2 em `eu-central-1` (Frankfurt).

Isso fecha, de uma vez, os buracos 1 e 2 — porque a instância na AWS é
necessariamente outra máquina, com outra CPU, e o binário chega nela por upload.

Ordem de execução, e por que:

1. `just check-determinism` local — confirmar o core antes de gastar dinheiro.
2. `just aws-up lastblade2 <rom>` — sobe a infra, gera chave de sessão, envia
   binários e ROM, inicia o peer remoto.
3. Sessão com perfil `natural`. **Sem degradação sintética**: o objetivo é medir
   a rede de verdade, e injetar atraso por cima só embaralharia a medição.
4. Comparar o RTT medido com os perfis sintéticos, para saber quais deles
   representam o link real e quais eram pessimismo.
5. `just collect` — baixar os logs do peer remoto **antes** de destruir nada.
6. `just aws-down` — destruir tudo, e conferir com os comandos de
   [11 — Cleanup](11-cleanup.md).

O que se espera aprender, além do óbvio:

- se as regras de determinismo realmente aguentam duas CPUs diferentes
- qual é a forma da perda num link real, comparada com o modelo Bernoulli
- se a assimetria de rollback observada em loopback também aparece com a fase
  determinada por um link de verdade
- se 8 frames de limite de previsão bastam para a latência real
