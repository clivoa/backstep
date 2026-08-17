# 11 — Cleanup

## A ordem importa

```bash
just collect     # PRIMEIRO
just aws-down    # DEPOIS
```

`collect` baixa os logs do peer remoto do S3. `aws-down` destrói o bucket. Os
logs remotos **não existem em nenhum outro lugar** — a instância vai embora com
eles.

Por isso `aws-down` se recusa a rodar se `artifacts/logs` estiver vazio:

```
!!! artifacts/logs is empty. 'just collect' has not run.
!!! The remote logs are about to be destroyed with the bucket.
Run 'just collect' first, or re-run with FORCE=1 to discard them.
```

`FORCE=1 just aws-down` descarta conscientemente. É uma escolha, não um acidente.

## O que `aws-down` faz

1. Verifica se `collect` já rodou.
2. Esvazia o bucket explicitamente — **ROM, binários e logs remotos**.
3. `terraform destroy -auto-approve`.
4. Apaga a cópia local da chave de sessão (`artifacts/session.key`).

O passo 2 é redundante com `force_destroy = true`, e existe mesmo assim: o
bucket contém a ROM de alguém, e deixá-la para trás numa conta AWS não é
aceitável. Um `terraform destroy` que falhe por qualquer motivo não deve deixar a
ROM parada lá.

## Conferindo que sumiu

`aws-down` imprime os comandos no fim. Rode-os:

```bash
REGION=eu-central-1

# Nenhuma instância viva
aws ec2 describe-instances --region $REGION \
  --filters Name=tag:Project,Values=rollback-netcode \
            Name=instance-state-name,Values=running,pending,stopping,stopped \
  --query 'Reservations[].Instances[].InstanceId'

# Nenhum bucket
aws s3 ls | grep rollback-netcode

# Nenhum Elastic IP ocioso  <-- este é o que custa dinheiro
aws ec2 describe-addresses --region $REGION \
  --query 'Addresses[?AssociationId==`null`].[PublicIp,AllocationId]'

# Nenhum volume órfão
aws ec2 describe-volumes --region $REGION \
  --filters Name=status,Values=available \
  --query 'Volumes[].[VolumeId,Size]'

# Nenhuma VPC do laboratório
aws ec2 describe-vpcs --region $REGION \
  --filters Name=tag:Project,Values=rollback-netcode \
  --query 'Vpcs[].VpcId'

# O parâmetro da chave
aws ssm get-parameter --region $REGION --name /rollback-netcode/session-key \
  2>&1 | head -1
```

Todos devem retornar vazio, ou `ParameterNotFound` no último.

Todos os recursos têm a tag `Project=rollback-netcode`, então essa é a busca que
encontra qualquer coisa deixada para trás.

## Se o destroy falhar no meio

O modo de falha mais comum é o Terraform não conseguir apagar a VPC porque algo
ainda está preso a ela (uma ENI, um EIP). Rode de novo:

```bash
terraform -chdir=terraform destroy -auto-approve
```

Se persistir, o que costuma sobrar, em ordem de custo:

1. **Elastic IP não associado** — ~US$ 3,60/mês. Libere:
   `aws ec2 release-address --region eu-central-1 --allocation-id eipalloc-...`
2. **Volume EBS disponível** — `aws ec2 delete-volume --volume-id vol-...`
3. **Interface de rede** — `aws ec2 delete-network-interface --network-interface-id eni-...`
4. **Bucket com objetos** — `aws s3 rb s3://... --force`

Depois rode `terraform destroy` mais uma vez para o estado ficar consistente.

## Limpeza local

```bash
just clean-logs   # apaga artifacts/logs, artifacts/report e artifacts/e2e
just clean        # o acima + cargo clean
```

O que fica de propósito:

- `cores/fbneo_libretro.so` — leva 30 minutos para recompilar, não é apagado por
  descuido. Remova à mão se quiser.
- `terraform/terraform.tfstate` — apagar isso com recursos vivos **órfã tudo**.
  Só remova depois de confirmar que o destroy terminou.

## O que nunca esteve no repositório

- ROMs. `*.zip` está no `.gitignore` e nenhum passo do laboratório copia a ROM
  para dentro da árvore de fontes.
- Savestates. `artifacts/` inteiro está ignorado.
- Chaves. `session-key*` e `*.tfvars` (exceto `example.tfvars`) estão ignorados.
- Relatórios pessoais. Ficam em `artifacts/report/`, ignorado.

Vale conferir antes de publicar qualquer coisa:

```bash
git status --porcelain --ignored | grep '^!!' | head -20
```

## Checklist de fim de sessão

- [ ] `just collect` rodou e `artifacts/logs` tem arquivos dos **dois** peers
- [ ] `just aws-down` terminou sem erro
- [ ] Os seis comandos de verificação acima retornam vazio
- [ ] `artifacts/session.key` não existe mais
- [ ] `just local-down`, se a stack de observabilidade estiver rodando
