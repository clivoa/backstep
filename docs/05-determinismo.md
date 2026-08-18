# 05 — Determinismo

> *Ponto fixo*, *ULP*, *ASLR*, *savestate*, *NVRAM*: definições em
> [00 — Glossário](00-glossario.md).

## A regra

> Partindo do mesmo estado e recebendo os mesmos inputs, as duas máquinas
> precisam produzir estados **bit a bit idênticos**. Em toda máquina, em toda
> execução, em debug e em release.

Um único bit diferente e as duas simulações divergiram. Tudo depois disso é
ficção — os dois jogadores estão vendo jogos diferentes. Isso é *desync*.

Rollback não tolera desync melhor que lockstep. Pelo contrário: ele **depende**
de o replay reproduzir exatamente o que o replay original teria produzido.

## As regras que a arena obedece

A arena é código nativo e precisa impor cada regra à mão. (O emulador **não**
ganha determinismo de graça, ao contrário do que este documento afirmava antes
de a gente medir — ver [O emulador não era determinístico](#o-emulador-não-era-determinístico).)

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

## As regras que o emulador obriga

### O emulador não era determinístico

Esta seção começou como uma frase confiante — "o emulador é determinístico por
construção, o problema é só o ambiente ao redor" — e essa frase estava errada.
Vale contar como ela caiu, porque é o resultado mais instrutivo do laboratório.

O sintoma apareceu ao calibrar o script de boot do Last Blade 2: telas iguais
chegavam em frames diferentes a cada execução. Como o script é puramente
temporal, isso o quebrava de forma irreprodutível. O teste que decidiu:

```
# duas execuções, segundos diferentes
checksum f000300 e5467290974af991
checksum f000300 eaf4e5314c60aeed     <- divergiu

# duas execuções lançadas no MESMO segundo de relógio
checksum f000300 59c82db8a4071bd1
checksum f000300 59c82db8a4071bd1     <- idênticas
```

`time(NULL)` tem granularidade de um segundo. Duas execuções que caem no mesmo
segundo concordarem, e em segundos diferentes discordarem, é a assinatura
inequívoca de uma dependência do relógio da máquina.

Eram duas, ambas em `src/burn/burn.cpp` do FBNeo:

1. `BurnRandomInit()` semeia o RNG dos drivers com `time(NULL)`.
2. `BurnGetLocalTime()` devolve o calendário do host — e o Neo Geo tem um chip
   de relógio de calendário, o µPD4990A, que o BIOS lê durante o boot
   (`src/burn/drv/neogeo/neo_upd4990a.cpp:54`).

O conserto está em `docker/fbneo/determinism.md`: o FBNeo já resolve as duas
quando `kNetGame` está ligado, e o build do laboratório liga. O `Dockerfile`
falha ruidosamente se a linha que ele altera deixar de existir.

**A lição não é "FBNeo tem um bug".** É que "o emulador é determinístico" é uma
afirmação sobre o emulador *e sobre como ele é construído e configurado*, e a
única forma de saber é medir. Por isso existe:

```bash
just check-determinism /caminho/lastbld2.zip
```

Ele roda o core duas vezes, em processos separados, com um `sleep` deliberado
entre eles, e compara os checksums. O `sleep` é o teste: sem ele, duas execuções
caem no mesmo segundo e um core quebrado passa.

### NVRAM e configurações por jogo

FBNeo lê NVRAM e settings do diretório de sistema, e **escreve** ao descarregar.
No Neo Geo o arquivo é `<system>/fbneo/<romset>.fs`, o memory card, e ele guarda
créditos entre execuções.

Isso não é teórico: um peer que já rodou antes inicia com créditos inseridos,
chega à tela de título num frame diferente de um peer que nunca rodou, e os dois
scripts de boot apertam Start em momentos diferentes do loop de atração. O
resultado são duas máquinas em menus diferentes — que o rollback reporta,
corretamente, como desync.

Por isso `rollback_libretro::clear_persistent_state` apaga `.fs`, `.nv` e `.hi`
do jogo antes de carregar, dos dois lados, em toda sessão.

### O checksum estava medindo a coisa errada

Depois de tudo acima, o emulador ficou reprodutível **entre processos** — e as
sessões continuaram desincronizando, sempre no primeiro rollback. Zero desyncs
no perfil `natural` (que não faz rollback nenhum) e desync garantido em
`delay20` (que faz).

Rollback precisa de uma propriedade que "determinístico" não cobre: que
`retro_unserialize` restaure **tudo** que `retro_run` vai voltar a ler. Um core
pode ser perfeitamente reprodutível a partir de um boot frio e ainda guardar
estado fora do savestate.

A ferramenta que responde isso é `just check-rollback-safety`:

```
salva o estado no frame N
roda K frames com inputs I      -> checksum A     (peer que não voltou)
restaura, roda os mesmos K      -> checksum B     (peer que voltou)
A == B ?
```

O resultado, no Last Blade 2:

```
save -> load -> save discorda em 16 a 21 bytes de 415 155,
sempre em quatro campos de 4 bytes nos offsets 537, 829, 1413 e 1705.
```

E a pergunta que decide tudo — isso se espalha? Não:

```
probe no frame 2100, 300 frames re-simulados de luta
  -> 18 bytes diferentes, offset máximo 1761
probe no frame 2500 -> 23 bytes, offset máximo 1757
probe no frame 2900 -> 17 bytes, offset máximo 1499
```

Cinco segundos de luta re-simulada e a diferença continua sendo algumas dezenas
de bytes abaixo do offset 1800. Ela **nunca alcança** os 413 KB de RAM de
trabalho, RAM de vídeo e paleta onde o jogo de fato vive. São contadores de
timer e do chip de som, que o 68000 não lê de volta.

Ou seja: a máquina *era* segura para rollback; o checksum é que não era. Hashear
o blob inteiro reportava desync no primeiro rollback de toda sessão — um falso
positivo que torna o detector inútil justamente no core em que ele mais importa.

`CHECKSUM_SKIP_BYTES = 2048` é o conserto: o checksum ignora o prefixo instável.
O preço, dito com todas as letras, é que uma divergência real confinada a esses
2 KB passaria despercebida. Vale a pena porque a alternativa é um detector que
dispara sempre, e porque tudo que os jogadores conseguem observar vive depois
dessa fronteira. O `check-rollback-safety` **falha** se a instabilidade alcançar
o limite, então a afirmação é verificável e não uma esperança.

### O peer travado nunca percebia que o outro morreu

Achado de tabela, não de teoria: numa rodada com perda, um peer ficou vivo por
minutos acumulando 20 735 stalls num frame que nunca ia chegar.

A causa estava no `SessionRunner::step`: a checagem de liveness ficava no fim da
função, e o caminho "estou travado" fazia `return` antes disso. O peer que mais
precisava notar o silêncio era exatamente o único que não olhava.

Corrigido extraindo `check_peer_liveness` e chamando nos dois caminhos, com
regressão em `a_peer_that_dies_mid_session_times_out_while_stalled` — que mata o
peer *sem* mandar Disconnect, porque o caminho educado já funcionava.

### O BIOS entra no hash

Um jogo de Neo Geo é metade do código que roda; a outra metade é o `neogeo.zip`.
Dois peers com revisões diferentes de BIOS passariam pelo handshake e
divergiriam durante o boot, antes de qualquer input.

`app::hash_rom_set` hasheia ROM **e** BIOS, com separação de domínio, no mesmo
campo `rom_hash` do handshake. Um BIOS diferente agora é recusado na porta com
"ROM hash mismatch", que é uma mensagem chata mas honesta.

### Opções do core

Algumas opções do FBNeo mudam quantos ciclos de máquina um frame executa.
Aquelas são fixadas explicitamente em `PINNED_CORE_OPTIONS`, em vez de ficarem no
que o core resolver usar como padrão naquela máquina:

| Opção | Valor | Por quê |
|---|---|---|
| `fbneo-frameskip` | `0` | Frameskip faria `retro_run` avançar um número variável de frames; rollback assume exatamente um |
| `fbneo-cpu-speed-adjust` | `100` | Muda o orçamento de ciclos por frame |
| `fbneo-neogeo-mode` | `DIPSWITCH` | Fixa a região emulada, que muda a taxa de frames |
| `fbneo-diagnostic-input` | `Disabled` | O menu de serviço não pode ser aberto por acidente |

A última linha era `Hold Start`, e trocá-la foi uma tentativa de explicar por que
o script de boot não conseguia iniciar a partida. **Não era a causa** — a causa
real está em [09](09-sfa3.md): a placa quer o Start *segurado* por ~75 frames.
Mesmo assim `Disabled` é o valor certo: o script segura Start por 120 frames, e
não faz sentido deixar um gesto de diagnóstico apontado para exatamente isso.

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
