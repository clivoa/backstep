# 10 — Custos

Preços de `eu-central-1` (Frankfurt), on-demand, em USD, como referência de
ordem de grandeza. Confira a tabela atual da AWS antes de planejar qualquer coisa
— preços mudam e este arquivo não.

## Uma sessão de 4 horas

| Recurso | Preço | 4 h |
|---|---|---|
| EC2 `t3.small` on-demand | ~0,0216 /h | ~0,086 |
| EBS gp3 20 GB | ~0,0952 /GB-mês | ~0,010 |
| Elastic IP (associado a instância ligada) | grátis | 0,00 |
| S3 Standard, ~200 MB | ~0,0245 /GB-mês | <0,01 |
| Requisições S3 (algumas centenas) | ~0,005 /1000 PUT | <0,01 |
| SSM Parameter Store (tier Standard) | grátis | 0,00 |
| SSM Session Manager | grátis | 0,00 |
| Transferência de saída, sessão de 180 s | ~0,09 /GB | desprezível |

**Total: cerca de US$ 0,11 por sessão de 4 horas.**

A transferência merece uma nota, porque contraria a intuição: uma sessão de 180 s
a ~35 kbit/s move cerca de **0,8 MB** por direção. Só inputs trafegam. Você não
consegue gastar dinheiro relevante com rede neste laboratório nem tentando.

## O que realmente custa dinheiro

### Uma instância esquecida

`t3.small` ligada por um mês inteiro: **~US$ 15**, mais o volume. É a única forma
plausível de este laboratório custar algo perceptível.

Duas defesas, as duas ativas por padrão:

```hcl
instance_initiated_shutdown_behavior = "terminate"
```

```bash
shutdown -h "+240"    # armado no boot pelo user-data
```

Como o comportamento é *terminate* e não *stop*, o prazo de 4 horas é real: a
instância desaparece, e o volume vai junto (`delete_on_termination = true`).

### Elastic IP não associado

Um EIP **associado a uma instância em execução** é grátis. Um EIP alocado e
ocioso custa ~US$ 3,60/mês.

Isso importa se um `terraform destroy` falhar no meio: a instância pode sumir e o
EIP ficar. É por isso que [11 — Cleanup](11-cleanup.md) tem uma verificação
explícita de EIP.

### Volumes órfãos

Um `delete_on_termination = true` cobre o caso normal. Um snapshot criado à mão,
ou um volume de uma instância terminada por outro caminho, não.

### Bucket com objetos

O bucket tem `force_destroy = true` e lifecycle de 7 dias, e `just aws-down`
apaga os objetos explicitamente antes do destroy. Mas 200 MB parados custam
centavos por mês — o problema aqui não é dinheiro, é a **ROM** ficar num bucket
esquecido.

## Estimando antes de subir

```bash
just aws-plan
```

O plano lista os 21 recursos. Os que geram custo contínuo são: `aws_instance`,
`aws_ebs_volume` (via `root_block_device`), `aws_eip` e `aws_s3_bucket`.

## Custo de execução, o outro tipo

Além da fatura, há o custo de CPU do rollback. Ele aparece no dashboard e no
`summary.csv`:

| Métrica | O que significa |
|---|---|
| `resimulation_overhead` | Frames re-simulados por frame apresentado. 0,1 = 10% de trabalho extra. |
| `rollback_advance_seconds_total` | Tempo somado dentro de `advance_frame`. |
| `rollback_save_state_seconds_total` | Tempo somado dentro de `save_state`. |
| `cpu_seconds` | CPU do processo, de `/proc/self/stat`. |
| `effective_fps` | Frames apresentados ÷ duração. Precisa ficar perto de 60. |

Na arena, com estado de 204 bytes, esses números são ruído. No SFA3 são a coisa
que decide entre `t3.small` e `t3.medium`.

## Comparação: por que não algo maior

`t3.medium` custa o dobro (~0,0432/h). Para a arena é desperdício puro. Para o
SFA3, vale medir primeiro: o gargalo é `retro_serialize`, que é dominado por
largura de banda de memória e não por número de vCPUs, então o ganho de dobrar
o tipo pode ser menor do que parece. O caminho certo é olhar
`rollback_save_state_seconds_total` antes de trocar.

## Regra prática

Rode `just collect && just aws-down` no fim de **toda** sessão. O laboratório
inteiro custa menos que um café enquanto isso for verdade, e é isso que os dois
mecanismos de shutdown existem para garantir mesmo quando não é.
