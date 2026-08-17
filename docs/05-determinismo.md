# 05 — Determinismo

## A regra

> Partindo do mesmo estado e recebendo os mesmos inputs, as duas máquinas
> precisam produzir estados **bit a bit idênticos**. Em toda máquina, em toda
> execução, em debug e em release.

Um único bit diferente e as duas simulações divergiram. Tudo depois disso é
ficção — os dois jogadores estão vendo jogos diferentes. Isso é *desync*.

Rollback não tolera desync melhor que lockstep. Pelo contrário: ele **depende**
de o replay reproduzir exatamente o que o replay original teria produzido.

## As regras que a arena obedece

O SFA3 ganha determinismo de graça: o emulador é uma máquina de estados
determinística por construção. Código nativo precisa impor cada regra à mão.

### 1. Nenhum ponto flutuante

Não porque inteiros sejam mais rápidos — nesta escala não são — mas porque a
soma em `f32` é apenas *quase sempre* reprodutível:

- o compilador pode contrair `a * b + c` em FMA, mudando o arredondamento;
- x87 tem precisão excedente em registradores;
- reassociação estilo `-ffast-math` é legal sob certas flags do LLVM;
- bibliotecas matemáticas diferem entre plataformas em funções transcendentais.

Uma diferença de um ULP em um peer é um desync. Inteiros não têm nenhum desses
graus de liberdade.

A arena usa ponto fixo Q23.8: um `i32`, oito bits fracionários, unidade de 1/256
de pixel. A multiplicação passa por `i64` para o produto intermediário não
transbordar, e o deslocamento de volta é aritmético — arredonda para menos
infinito, consistentemente, em toda plataforma. Ver `crates/rollback-arena/src/fixed.rs`.

### 2. Nenhum hash, nenhuma iteração sobre `HashMap`

A ordem de iteração de um `HashMap` em Rust depende de uma semente aleatória por
processo. Dois peers iterariam em ordens diferentes. Tudo que precisa ser
determinístico usa `BTreeMap`, `Vec` de tamanho fixo, ou array.

Pelo mesmo motivo, os hashes que atravessam a rede usam FNV-1a implementado no
repositório (`Fnv1a`), e não `DefaultHasher` — que não garante estabilidade nem
entre versões do Rust.

### 3. Nenhum valor derivado de ponteiro

Endereços de memória variam com ASLR. Nada na simulação pode depender deles.

### 4. Nenhum relógio, nenhuma thread, nenhuma aleatoriedade

A simulação não lê o relógio, não observa escalonamento de threads e não sorteia
nada. O `DeterministicRng` do repositório existe para os **bots** e para o
emulador de rede — que são jogadores e infraestrutura, não simulação.

### 5. Todo laço tem contagem e ordem fixas

O pool de projéteis tem 4 slots, sempre percorridos na mesma ordem. Os dois
lutadores são sempre processados na ordem 0, 1. A separação de corpos dá a
unidade ímpar de ponto fixo sempre ao lutador 0 — arbitrário, mas **fixo**, então
os dois peers fazem a mesma escolha.

### 6. `overflow-checks` ligado também em release

```toml
[profile.release]
overflow-checks = true
```

Em release, o Rust por padrão faz aritmética wrapping; em debug, entra em pânico.
Um overflow que ocorresse só em release produziria valores diferentes dos de
debug — e, pior, produziria valores diferentes entre um peer compilado em debug
e outro em release. Ligar a checagem nos dois faz os dois se comportarem igual, e
transforma um overflow em falha ruidosa em vez de estado silenciosamente errado.

### 7. `OutputMode` não toca no estado

Detalhado em [02 — Arquitetura](02-arquitetura.md). Vídeo, áudio e contadores de
apresentação ficam **fora** do snapshot e **fora** do checksum. `presented_frames`
existe em `Arena` mas não é serializado nem hasheado, porque conta frames de
*tela* — que legitimamente diferem entre os peers.

## As regras que o SFA3 obriga

O emulador é determinístico. O que **não** é determinístico é o ambiente ao redor
dele:

### NVRAM e configurações por jogo

FBNeo lê NVRAM e settings do diretório de sistema. Um arquivo de NVRAM antigo em
um dos peers é desync no frame zero, antes de qualquer input. Por isso o host
expõe `set_directories`, e `just aws-up` aponta os dois lados para um diretório
limpo. Ver [09 — SFA3](09-sfa3.md).

### Opções do core

Algumas opções do FBNeo mudam quantos ciclos de máquina um frame executa.
Aquelas são fixadas explicitamente em `PINNED_CORE_OPTIONS`, em vez de ficarem no
que o core resolver usar como padrão naquela máquina:

| Opção | Valor | Por quê |
|---|---|---|
| `fbneo-frameskip` | `0` | Frameskip faria `retro_run` avançar um número variável de frames; rollback assume exatamente um |
| `fbneo-cpu-speed-adjust` | `100` | Muda o orçamento de ciclos por frame |
| `fbneo-neogeo-mode` | `DIPSWITCH` | Fixa a região emulada, que muda a taxa de frames |
| `fbneo-diagnostic-input` | `Hold Start` | Evita que uma combinação acidental abra o menu de serviço |

### O hash do core e da ROM

Emuladores diferentes simulam diferente. Revisões de ROM diferentes são jogos
diferentes. Os dois são comparados no handshake por SHA-256, e a sessão é
recusada com um motivo legível antes de começar.

## Como isso é verificado

### Replay de 100 000 frames, debug e release

`crates/rollback-arena/tests/replay_100k.rs` roda 100 000 frames de um script de
inputs determinístico e afirma um checksum **constante fixado em código**:

```rust
const GOLDEN_SCRIPTED: u64 = 0xf594_92aa_1a1b_d8cf;
const GOLDEN_BOTS: u64 = 0x15fd_05bb_8237_0920;
```

A constante é o ponto. Sem ela, o teste só provaria que a arena concorda consigo
mesma naquele processo. Com ela, uma mudança que altere silenciosamente o
comportamento da simulação precisa ser **reconhecida** atualizando um número — e
isso é um sinal de que todo peer precisa ser recompilado antes de jogarem juntos.

`just test` roda esse arquivo em debug **e** em release. Se os dois discordarem,
alguma coisa na arena é sensível ao nível de otimização, o que é exatamente a
classe de bug que derruba uma sessão entre um peer compilado de cada jeito.

O mesmo arquivo também verifica que salvar e restaurar no meio do replay não muda
nada — a premissa central do rollback, testada a cada 997 frames (um primo, para
a interrupção cair em todas as fases dos ciclos internos da simulação).

### Testes de propriedade

`crates/rollback-core/tests/property_delivery.rs` gera entrega arbitrária de UDP
— reordenada, duplicada, atrasada — e afirma que a sessão converge no mesmo
estado de uma que recebeu tudo em ordem. A perda é modelada como atraso, porque é
nisso que o protocolo a transforma: a redundância de 8 inputs faz um datagrama
perdido significar "chega mais tarde", não "nunca chega".

> Esse arquivo encontrou um bug real: a condição de stall isentava um frame cujo
> input remoto tinha chegado fora de ordem, deixando a sessão avançar por cima de
> um buraco e precisar voltar mais fundo que o buffer de estados alcança. A
> profundidade agora é medida só a partir da fronteira contígua.

### Comparação de checksums em execução

A cada 60 frames confirmados, os peers trocam o checksum do estado. Um
desacordo encerra a sessão imediatamente e é registrado no JSONL como um evento
`desync` com os dois valores.

Isso é a rede de segurança, não a defesa. Quando um desync aparece, o trabalho é
descobrir qual das regras acima foi quebrada.

## Diagnosticando um desync

1. **Qual frame?** O evento `desync` no JSONL dá o número exato.
2. **Os dois peers rodam o mesmo commit?** O handshake garante isso, mas confira
   `app_commit` nos dois `session_start`.
3. **É reproduzível?** Rode `just bench` com a mesma semente e o mesmo perfil. Se
   reproduz, o problema está na simulação. Se não, procure algo dependente de
   tempo ou de escalonamento.
4. **Debug contra release.** Rode os dois peers em perfis diferentes de
   compilação. Se só assim desync, é otimização — ponto flutuante ou overflow.
5. **Isole na arena.** Se o desync é no SFA3, tente reproduzir na arena. Se a
   arena está limpa, o problema está no ambiente do core: NVRAM, opções, ou hash.
6. **Reduza para um teste.** O replay de 100 000 frames e os testes de propriedade
   são os lugares onde uma reprodução deve acabar morando.
