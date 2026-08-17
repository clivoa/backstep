# 02 — Arquitetura

## Mapa dos crates

```
rollback-core        motor de rollback: previsão, histórico, re-simulação, desync
 ├─ rollback-net     protocolo UDP versionado, HMAC, emulador de rede, métricas de link
 ├─ rollback-arena   simulação 2D determinística (só inteiros) + bot FSM
 ├─ rollback-libretro host libretro: retro_serialize/unserialize, boot do SFA3
 ├─ rollback-telemetry exportador Prometheus, log JSONL, amostragem de /proc
 └─ rollback-runner  cola: handshake + laço de frame compartilhado
     ├─ rollback-client  SDL2, humano no P1, overlay
     └─ rollback-bot     headless, FSM no P2, roda na EC2

rollback-report      lê os JSONL e produz summary.csv + report.html
fake-libretro-core   cdylib libretro real, para testar o FFI em CI sem ROM
```

Dependências apontam sempre para baixo. `rollback-core` não conhece rede, não
conhece libretro e não conhece telemetria — ele conhece a trait `Simulation` e
mais nada.

## A fronteira que importa: a trait `Simulation`

```rust
pub trait Simulation {
    fn save_state(&self) -> Vec<u8>;
    fn load_state(&mut self, data: &[u8]) -> Result<(), SimulationError>;
    fn advance_frame(&mut self, inputs: [PlayerInput; 2], output_mode: OutputMode);
    fn checksum(&self) -> u64;
}
```

Quatro métodos. É toda a superfície entre o motor de rollback e o que está sendo
simulado.

Essa fronteira é a tese do projeto. A arena e um emulador de arcade de 90 MB
implementam a mesma trait, e o `RollbackSession` é literalmente o mesmo código
nos dois casos. Se o motor precisasse saber alguma coisa sobre o jogo — posições,
hitboxes, o que quer que fosse — ele não conseguiria dirigir o FBNeo, cujo estado
é opaco por construção.

### `OutputMode`, e por que ele não pode afetar o estado

`advance_frame` recebe se o frame é `Present` (o jogador vai ver) ou `Resimulate`
(replay de correção). O contrato é estrito:

> `OutputMode` pode alterar **saída** — vídeo, áudio, vibração, log — e **nunca**
> o estado da simulação.

Um rollback de 8 frames roda 8 `advance_frame` dentro de um único frame de tela.
Se o vídeo não fosse descartado, o jogador veria um borrão; se o áudio não fosse,
ouviria 8 frames de som de uma vez. Mas se `OutputMode` mudasse o estado, os dois
peers divergiriam — porque eles re-simulam frames diferentes.

Isso é testado explicitamente em três lugares:
`output_mode_does_not_touch_simulation_state` (core),
`output_mode_cannot_touch_simulation_state` (arena) e
`output_mode_does_not_change_the_machine_state` (libretro, via FFI real).

## O ciclo de um frame

O laço vive em `rollback-runner/src/runner.rs`. A ordem é deliberada:

```
1. receive        drena o socket; inputs remotos entram primeiro, então um
                  rollback acontece ANTES de construirmos mais coisa em cima
2. would_stall?   se a janela de previsão encheu, nenhum trabalho local
3. ler input      controle humano ou FSM do bot
4. send           o batch sai ANTES de simular, para o peer ganhar um frame
5. advance        simula e apresenta
6. checksums      qualquer frame que virou final tem o checksum trocado
7. telemetria     publica, loga, checa o timeout do peer
```

Cada passo tem um motivo:

- **1 antes de 5** porque aplicar inputs antigos depois de já ter especulado o
  frame atual só aumentaria a profundidade do rollback.
- **2 antes de 3** porque enfileirar um input local durante um stall re-arquivaria
  o mesmo frame no tick seguinte; se o primeiro valor já tivesse ido para o cabo,
  o peer veria dois inputs diferentes para um frame e rejeitaria a sessão. Isso
  é o erro `LocalInputRefiled`.
- **4 antes de 5** porque simular leva tempo, e esse tempo é latência pura para
  o peer.

## Onde cada preocupação mora

| Preocupação | Onde | Por que ali |
|---|---|---|
| Previsão e rollback | `rollback-core::session` | Não depende de rede nem de jogo |
| Regra de previsão | `session::predict_remote` | Uma função, trocável |
| Formato do datagrama | `rollback-net::wire` | Testável byte a byte sem socket |
| Autenticação | `rollback-net::auth` | Separada do formato de propósito |
| Atraso/perda sintéticos | `rollback-net::emulator` | Aplicado na **saída** (ver 03) |
| RTT, perda, bitrate | `rollback-net::link` | Medição, não transporte |
| Física da arena | `rollback-arena::arena` | Só inteiros, sem RNG |
| Bot da arena | `rollback-arena::bot` | É um **jogador**, não parte da simulação |
| FFI libretro | `rollback-libretro::{ffi,host,core}` | Único lugar com `unsafe` |
| Boot do SFA3 | `rollback-libretro::sfa3` | Macros temporizadas, sem offsets de ROM |
| Exportador/JSONL | `rollback-telemetry` | Uma fonte, três consumidores |
| Handshake | `rollback-runner::handshake` | Compatibilidade, não segurança |

## Por que os bots não são parte da simulação

Tanto `ArenaBot` quanto `Sfa3Bot` produzem um `PlayerInput` por frame, exatamente
como um controle faria. O input viaja pelo cabo como qualquer outro.

Isso importa: se o bot fosse parte da simulação, os dois peers precisariam
executá-lo de forma idêntica, e o gerador aleatório dele viraria mais uma fonte
possível de desync. Como ele é um *jogador*, a aleatoriedade dele é irrelevante
para a sincronia — ela só precisa ser determinística para que `just bench` seja
repetível a partir da semente.

O `Sfa3Bot` tem uma restrição adicional: ele **não lê nada do jogo**. Não pode,
sem offsets de memória da ROM, que este laboratório proíbe deliberadamente
(o porquê está em [09 — SFA3](09-sfa3.md)). Ele toca um repertório fixo de macros.

## `unsafe`: onde está e por quê

Todos os crates declaram `#![forbid(unsafe_code)]`, com duas exceções:

- **`rollback-libretro`** — carrega uma biblioteca C via `dlopen` e a chama.
  Não há como fazer isso com segurança em Rust puro. A mitigação é o handshake:
  os dois peers comparam o SHA-256 do core antes de começar, então um core
  trocado ou corrompido vira conexão recusada em vez de comportamento indefinido.
- **`fake-libretro-core`** — implementa a mesma ABI C, para poder ser carregado
  pelo host nos testes.

Há também um `unsafe` isolado em `rollback-client/src/render.rs`, para
reinterpretar o framebuffer `u32` como bytes na hora de atualizar a textura SDL.
Está documentado no ponto de uso, e a alternativa seria uma dependência inteira
(`bytemuck`) para uma função de três linhas.

## Testes: o que cada camada prova

| Camada | Teste | O que garante |
|---|---|---|
| `rollback-core` | unitários + `property_delivery.rs` | Convergência sob entrega arbitrária de UDP |
| `rollback-arena` | `replay_100k.rs` | Mesmo checksum em debug e release, 100 000 frames |
| `rollback-net` | `golden_protocol.rs` | Bytes exatos do formato e do HMAC |
| `rollback-libretro` | `fake_core_ffi.rs` | Caminho FFI real, sem ROM |
| `rollback-runner` | testes de par | Dois peers reais sobre UDP de verdade |
| Sistema | `ops/scripts/e2e-local.sh` | Dois **processos**, cinco perfis, zero desync |

Os testes de propriedade e o E2E não são decoração: cada um encontrou um bug
real durante a implementação, ambos documentados no histórico de commits.
