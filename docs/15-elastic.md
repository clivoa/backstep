# 15 — Elastic: as perguntas que o `summary.csv` não responde

> Termos como *profundidade*, *SRTT*, *stall* e *perfil* estão definidos em
> [00 — Glossário](00-glossario.md).

## Por que mais uma ferramenta

O laboratório já tem duas formas de olhar para uma sessão:

| Ferramenta | Granularidade | Boa para |
|---|---|---|
| Prometheus + Grafana ([07](07-dashboard.md)) | ao vivo, ~1 s | ver a sessão **enquanto** ela roda |
| `summary.csv` / `report.html` | uma linha por sessão | **comparar** sessões e perfis |
| **Elastic** | um documento por evento | **investigar** uma sessão específica |

O `summary.csv` diz "profundidade média 2,04". Ele não diz que 94% dos rollbacks
foram de profundidade 2 e exatamente quatro chegaram a 4 — que é a informação
que responde se o limite de previsão de 8 está sendo testado ou nem chega perto.

Os logs JSONL sempre tiveram esse detalhe. Faltava um lugar onde consultá-lo.

## Subir e carregar

```bash
just elastic-up          # Elasticsearch :9200, Kibana :5601, ambos em loopback
just elastic-load        # indexa artifacts/logs
just elastic-analyze     # as análises de sempre, direto no terminal
just elastic-down
```

O carregador cria as *data views* do Kibana sozinho, então
`http://127.0.0.1:5601` abre pronto para consultar.

Tudo em loopback e sem senha, pelo mesmo motivo do exportador de métricas: é
ferramenta de análise numa máquina, não serviço exposto. Ver [06 — AWS](06-aws.md)
para o mesmo argumento.

## Os dois índices

| Índice | Um documento por | Use para |
|---|---|---|
| `rollback-metrics` | segundo, por peer | gráficos, percentis, tendências |
| `rollback-events` | evento de sessão | forense frame a frame |

Registros de datagrama (`sent`, `received`, `local_input`, `remote_inputs`) são o
grosso de um log — dezenas de milhares de linhas cada — e ficam de fora por
padrão. `just elastic-load artifacts/logs 1` inclui todos.

Cada documento carrega a identidade da sessão (simulação, perfil, jogador,
semente, commit) desnormalizada, então nada precisa de junção para ser filtrado.

### Duas decisões de mapeamento que importam

**Checksums são `keyword`, não número.** São FNV-1a de 64 bits **sem sinal**, e
não cabem no `long` com sinal do Elasticsearch — a primeira tentativa de carga
falhou com `Numeric value out of range of long`. Guardá-los truncados seria pior
que inútil: dois estados diferentes poderiam colidir. Além disso, ninguém tira
média de checksum; compara. `keyword` em hexadecimal é a representação certa.

**Métricas derivadas são calculadas na carga**, não no Kibana:
`derived.effective_fps`, `derived.resimulation_overhead`, `derived.loss_pct`,
`derived.srtt_ms`. Assim todo consumidor do índice concorda com o `summary.csv`
sem re-derivar nada — e sem a chance de duas pessoas derivarem diferente.

## O que isso revelou

Coisas que estavam nos logos o tempo todo e que nenhuma tabela mostrava.

### 1. Jitter não aumenta a profundidade — ele **espalha** a distribuição

Esta é a descoberta mais valiosa. Comparando os perfis, no peer que trabalha:

| Perfil | Rollbacks | Distribuição de profundidade |
|---|---|---|
| `loss2` | 5 | `d1: 100%` |
| `natural` | 26 | `d1: 23%  d2: 76%` |
| `delay20` | 260 | `d4: 11%  d5: 88%` |
| `jitter30` | 260 | `d4: 13%  d5: 59%  d6: 27%` |
| `combined` | 259 | `d3: 1%  d4: 39%  d5: 47%  d6: 10%  d7: <1%` |

`delay20` e `jitter30` produzem o **mesmo número** de rollbacks e médias quase
iguais — foi o que [08](08-experimentos.md) concluiu, e continua certo. Mas a
forma é diferente: `delay20` é concentrado (88% num único valor), `jitter30` é
espalhado, e é o espalhamento que empurra a **cauda** de 5 para 6.

Isso é uma afirmação precisa sobre o que jitter faz ao rollback, e a média era
incapaz de expressá-la: **jitter não custa mais trabalho, custa pior caso.** E o
pior caso é o que encosta no limite de previsão e vira stall.

### 2. Perda produz só a correção mais rasa possível

`loss2`: **100% dos rollbacks em profundidade 1**. Faz sentido — um input perdido
chega no datagrama seguinte, 16,7 ms depois, então a correção nunca precisa
voltar mais de um frame.

É a confirmação mais direta possível de que a redundância de oito inputs faz o
que foi feita para fazer, e de que perda e latência são problemas diferentes.

### 3. A cauda do RTT, que a média esconde

Na sessão real Madri ↔ Frankfurt:

```
p50  49,97 ms      p90  52,21 ms      p99  54,96 ms      max  57,11 ms
```

O SRTT reportado era 49,9 ms. O pior caso foi 57,1 — e no outro peer, 72,4 ms.
Um pico de 22 ms acima da mediana que nenhuma média mostraria.

### 4. Onde o orçamento de frame realmente vai

Nanossegundos acumulados divididos por frames apresentados, ou seja, média por
frame:

| Sessão | `advance` | `save_state` | `load_state` |
|---|---|---|---|
| Last Blade 2 (Madri) | **3 948 µs** | **2 271 µs** | 17 µs |
| arena (Madri) | 1,3 µs | 0,9 µs | 0,0 µs |

Um frame a 60 Hz tem **16 667 µs**. O emulador gasta 6,2 ms deles — 37% —
apenas em avançar e salvar. A arena gasta 2,2 µs, três ordens de grandeza menos.

É o argumento de custo do rollback quantificado: `save_state` sozinho, num
estado de 415 KB, come 14% do orçamento **em todo frame**, tenha havido rollback
ou não.

## Consultas úteis

O `just elastic-analyze` roda as quatro acima. Para as suas próprias, no
Dev Tools do Kibana:

```json
// Quando os rollbacks se agruparam?
GET rollback-events/_search
{
  "size": 0,
  "query": { "bool": { "filter": [
    { "term": { "event": "rolled_back" } },
    { "term": { "profile": "combined" } }
  ]}},
  "aggs": { "no_tempo": {
    "date_histogram": { "field": "@timestamp", "fixed_interval": "5s" },
    "aggs": { "profundidade_media": { "avg": { "field": "depth" } } }
  }}
}
```

```json
// A profundidade subiu antes do stall?
GET rollback-metrics/_search
{
  "size": 20,
  "query": { "range": { "local.stalls": { "gt": 0 } } },
  "sort": [{ "@timestamp": "asc" }],
  "_source": ["@timestamp", "frame", "prediction_depth",
              "local.stalls", "derived.srtt_ms"]
}
```

```json
// Os dois peers concordaram em todo checksum comparado?
GET rollback-events/_search
{
  "size": 0,
  "query": { "term": { "event": "checksum_matched" } },
  "aggs": { "por_sessao": { "terms": { "field": "session", "size": 50 } } }
}
```

## Limitações

- **É análise post-mortem.** O carregamento é manual, depois da sessão. Não há
  envio ao vivo — para isso existe o Prometheus.
- **Sem retenção.** Os índices crescem até você rodar `just elastic-reload`. Uma
  sessão de 5 minutos são ~20 000 documentos sem os datagramas, ~60 000 com.
- **Sem dashboard versionado.** As *data views* são criadas automaticamente; um
  dashboard salvo do Kibana seria um JSON grande e frágil entre versões, e
  envelheceria pior que as consultas acima.
