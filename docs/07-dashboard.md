# 07 — Dashboard

> O que cada métrica significa e como lê-la está em
> [00 — Glossário: métricas](00-glossario.md#as-métricas-que-o-laboratório-reporta).

## Subir

```bash
just local-up
```

- Grafana: <http://127.0.0.1:3000> (acesso anônimo, dashboard já provisionado)
- Prometheus: <http://127.0.0.1:9090>
- Exportador: <http://127.0.0.1:9898/metrics> (só existe com uma sessão rodando)

Numa bancada local com dois peers, o segundo exporta em `127.0.0.1:9899` e o
Prometheus raspa os dois, com os labels `instance="local"` e
`instance="local-peer2"`.

Tudo escuta em loopback. Isso é intencional e está explicado em
[06 — AWS](06-aws.md).

## Como o peer remoto aparece

O Prometheus **não** raspa a EC2. Não há porta de métricas aberta lá.

Cada peer manda ao outro um `TelemetrySummary` a cada 60 frames pelo próprio link
da sessão. O exportador local re-publica esses números com o label
`peer="remote"`, ao lado dos seus com `peer="local"`.

Consequência prática: `rollback_rollbacks_total{peer="remote"}` é **o que o peer
remoto diz sobre si mesmo**, atrasado de até um segundo. É a informação certa
para comparar os dois lados, e não é a mesma coisa que raspar a instância.

## O que cada painel significa

### Sessão

| Painel | Leitura |
|---|---|
| **Desync** | 0 = ok. 1 = dois checksums de frames confirmados divergiram e a sessão acabou. Não há estado intermediário. |
| **Profundidade de previsão** | Quantos frames à frente do peer estamos especulando. O limite é 8. Encostar nele = stall. |
| **Acurácia da previsão** | Fração dos chutes que se confirmaram. Abaixo de ~0,85 sob latência moderada indica que o adversário está mexendo muito, ou que o link piorou. |
| **RTT suavizado** | SRTT do RFC 6298. Não existe latência unidirecional aqui — ver [03](03-protocolo.md). |
| **Tamanho do estado** | 204 bytes na arena; alguns megabytes no SFA3. |

### Rollback

| Painel | Leitura |
|---|---|
| **Rollbacks por segundo** | Cada um é uma previsão que não se confirmou. Compare os dois peers: quem está atrás corrige mais. |
| **Profundidade do rollback** | Média (frames re-simulados ÷ rollbacks) e o máximo já visto. O máximo não pode passar do limite de previsão. |
| **Trabalho extra de simulação** | Frames re-simulados por frame apresentado. 0 = nenhum rollback; 1 = a CPU simulou tudo duas vezes. É o custo de CPU do rollback, direto. |
| **Stalls** | Frames em que a janela encheu e a simulação parou. Diferente de zero significa que o peer não está acompanhando. |

### Rede

| Painel | Leitura |
|---|---|
| **RTT e variação** | Sob `jitter30`, a variação é o número interessante. |
| **Perda, duplicação, reordenação** | A perda é **inferida** por lacunas de sequência; um datagrama atrasado aparece como perda até chegar, e então a estimativa se corrige. |
| **Bitrate** | Só inputs trafegam. Um `InputBatch` a 60 Hz com 8 inputs repetidos dá ~35 kbit/s. Se estiver muito acima, algo está mandando mais do que deveria. |
| **Datagramas rejeitados** | Falhas de HMAC e pacotes malformados. Diferente de zero fora de teste significa que alguém está mandando lixo na porta. |

### Custo de execução

| Painel | Leitura |
|---|---|
| **Tempo por frame** | Quanto de cada frame vai em `advance_frame`, `save_state` e `load_state`. A 60 Hz o orçamento é 16,7 ms; queira ficar abaixo de ~8 ms para caber o pior caso de rollback. |
| **CPU e memória** | Lidos de `/proc/self/stat` e `/proc/self/statm` a cada 30 frames. |

## Assinaturas visuais

**Sessão saudável sob `delay20`:** profundidade de previsão estável em 2–3,
acurácia acima de 0,9, rollbacks constantes mas rasos (profundidade média perto de
1–2), stalls em zero, perda em zero.

**`loss2` funcionando como projetado:** perda inferida oscilando perto de 2%, e os
rollbacks **não** acompanhando. É a redundância de 8 inputs fazendo o trabalho: o
input perdido chega no datagrama seguinte, antes de ser necessário.

**O peer não está acompanhando:** profundidade de previsão colada em 8, stalls
subindo, e `effective_fps` abaixo de 60. Ou a rede piorou, ou a instância é
pequena demais para a simulação.

**Desync:** o painel vira vermelho e todos os contadores param. A sessão acabou;
o JSONL tem o evento com os dois checksums e o número do frame.

## Consultas úteis

```promql
# custo do rollback em CPU
rate(rollback_frames_resimulated_total[30s])
  / clamp_min(rate(rollback_frames_presented_total[30s]), 0.001)

# os dois peers lado a lado
rollback_rollbacks_total

# quanto do orçamento de frame está sendo usado
rate(rollback_advance_seconds_total[15s])
  / clamp_min(rate(rollback_frames_presented_total[15s])
            + rate(rollback_frames_resimulated_total[15s]), 0.001)

# a sessão está de fato a 60 Hz?
rate(rollback_frames_presented_total{peer="local"}[30s])
```

## O dashboard é versionado

`ops/grafana/dashboards/rollback.json` é montado somente-leitura, com
`allowUiUpdates: false` e `disableDeletion: true`. O dashboard faz parte do
experimento; ele não deve derivar no navegador de alguém e depois não ser
reproduzível.

Para alterá-lo: edite o JSON, `just local-down && just local-up`, e commite.

## O relatório HTML

O dashboard mostra o *agora*. O relatório mostra o que **aconteceu**:

```bash
just report   # a partir dos logs já em disco
```

Produz `artifacts/report/report.html`, autocontido: sem CDN, sem script, sem
fonte externa, gráficos em SVG inline. Precisa ser legível de um laptop com o
Wi-Fi desligado, meses depois de a conta AWS ter sido destruída.

Ele traz uma visão geral por perfil, uma tabela com os dois peers lado a lado
para cada execução, séries temporais, e uma seção de ressalvas sobre como ler os
números.
