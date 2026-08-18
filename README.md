# Laboratório de Rollback Netcode — Madrid ⇄ AWS Frankfurt

Um workspace Rust que demonstra rollback netcode em dois níveis:

1. **Arena 2D** determinística e totalmente instrumentada — cada byte do estado
   é auditável, o snapshot tem 204 bytes e o checksum cobre tudo.
2. **Street Fighter Alpha 3** rodando no core oficial FBNeo via API libretro,
   usando exatamente o mesmo motor de rollback.

O jogador local controla P1 por SDL2. Uma EC2 headless em Frankfurt controla P2
por FSM. Os peers trocam **apenas inputs** por UDP autenticado com HMAC-SHA256,
enquanto Prometheus, Grafana, JSONL e um relatório HTML registram conectividade,
previsões, rollbacks e desyncs.

---

## Índice da documentação

| Documento | Assunto |
|---|---|
| [01 — Teoria](docs/01-teoria.md) | O que é rollback, por que existe, o que ele custa |
| [02 — Arquitetura](docs/02-arquitetura.md) | Os oito crates e por que a fronteira está onde está |
| [03 — Protocolo](docs/03-protocolo.md) | Formato de datagrama, autenticação, handshake |
| [04 — Uso local](docs/04-uso-local.md) | Controles, comandos, sessão local ponta a ponta |
| [05 — Determinismo](docs/05-determinismo.md) | As regras que impedem desync, e como são verificadas |
| [06 — AWS](docs/06-aws.md) | A infraestrutura, o modelo de ameaça, a chave de sessão |
| [07 — Dashboard](docs/07-dashboard.md) | Prometheus, Grafana e o que cada painel significa |
| [08 — Experimentos](docs/08-experimentos.md) | Os cinco perfis, o método, os resultados |
| [09 — Jogos reais](docs/09-sfa3.md) | O core FBNeo, o boot determinístico, o commit pinado |
| [10 — Custos](docs/10-custos.md) | Quanto custa uma sessão, e onde o dinheiro some |
| [11 — Cleanup](docs/11-cleanup.md) | Como destruir tudo e conferir que sumiu |
| [12 — Troubleshooting](docs/12-troubleshooting.md) | Sintomas, causas e o que olhar primeiro |

---

## Começo rápido

### Pré-requisitos

| Ferramenta | Para quê | Verificar |
|---|---|---|
| Rust ≥ 1.82 | compilar tudo | `cargo --version` |
| SDL2 ≥ 2.0.20 | cliente gráfico | `pkg-config --modversion sdl2` |
| Docker | build do FBNeo, Prometheus, Grafana | `docker --version` |
| `just` | os comandos abaixo | `just --version` |
| Terraform ≥ 1.6 | infraestrutura AWS | `terraform version` |
| AWS CLI | `aws-up`, `collect`, `aws-down` | `aws sts get-caller-identity` |
| shellcheck | gate de lint dos scripts | `shellcheck --version` |

Arquitetura x86_64. O `docker-compose` da observabilidade usa rede de host e
portanto é **Linux-only** — o motivo está explicado no próprio arquivo.

### Um teste completo, sem AWS e sem ROM

```bash
just test        # fmt, clippy, testes (debug e release), shellcheck, terraform
just e2e         # dois processos reais, socket real, os cinco perfis
just bench       # 180 s por perfil, gera summary.csv e report.html
```

`just bench` produz:

- `artifacts/report/summary.csv` — uma linha por sessão, ~37 colunas
- `artifacts/report/report.html` — autocontido: sem CDN, sem script, gráficos
  em SVG inline

### Observabilidade local

```bash
just local-up
# Grafana     http://127.0.0.1:3000
# Prometheus  http://127.0.0.1:9090
# Exportador  http://127.0.0.1:9898/metrics
```

### Sessão contra a AWS

```bash
cp terraform/example.tfvars terraform/terraform.tfvars
$EDITOR terraform/terraform.tfvars     # allowed_cidr = seu IP/32
curl -s https://checkip.amazonaws.com  # para descobrir o IP

just aws-up arena
just play arena
just collect      # SEMPRE antes do aws-down
just aws-down
```

Para um jogo de arcade é preciso fornecer a própria ROM:

```bash
just build-core                                  # compila o FBNeo em container

# The Last Blade 2 (Neo Geo). Precisa também de neogeo.zip, o BIOS,
# em artifacts/system/ — é metade do código que roda.
just check-determinism /caminho/lastbld2.zip     # confira o core antes
just e2e 90 lastblade2 /caminho/lastbld2.zip
just aws-up lastblade2 /caminho/lastbld2.zip
just play lastblade2 /caminho/lastbld2.zip

# Street Fighter Alpha 3 (CPS-2). O set precisa conter sfa3.key.
just aws-up sfa3 /caminho/sfa3.zip
```

O `just check-determinism` não é opcional por preciosismo: o FBNeo, como vem,
semeia RNG e relógio emulado a partir do relógio do host, e dois peers que
iniciam em segundos diferentes divergem antes do primeiro input. O `just
build-core` corrige isso; ver [05 — Determinismo](docs/05-determinismo.md).

---

## O que este repositório **não** contém

- **Nenhuma ROM e nenhum BIOS.** `sfa3.zip`, `lastbld2.zip` e `neogeo.zip` são
  fornecidos por você e nunca são versionados, redistribuídos nem incluídos em
  qualquer artefato deste repositório.
- **Nenhum savestate ou log pessoal.** `artifacts/` está no `.gitignore`.
- **Nenhuma chave.** A chave de sessão é efêmera, gerada por execução, guardada
  em SSM SecureString e num arquivo local modo 0600, e nunca entra no estado do
  Terraform nem em linha de comando.

## Fora do escopo do MVP

STUN, relay, matchmaking, espectador, reconexão, sincronização de estado,
Tekken 3, IA por visão, bot por leitura de memória, e múltiplas regiões.
O Fightcade é referência comparativa, não dependência.

## Sobre o idioma

O código e as APIs estão em inglês; a documentação didática está em português.
O enunciado do laboratório pede as duas coisas em pontos diferentes ("documentação
em português cobrindo teoria…" e "documentação didática em inglês"); a escolha
aqui seguiu o critério de aceitação, que é mais específico. Os comentários dentro
do código continuam em inglês, junto do que explicam.
