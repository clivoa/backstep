# 06 — AWS

## O que sobe

21 recursos, todos em `terraform/`:

```
VPC 10.42.0.0/16
 └─ subnet pública 10.42.1.0/24  ──  Internet Gateway  ──  route table
     └─ EC2 t3.small, Ubuntu 24.04 x86_64
         ├─ Elastic IP                    endereço estável entre rebuilds
         ├─ volume gp3 20 GB criptografado, delete_on_termination
         ├─ security group                UDP/7000, de um /32, e nada mais
         ├─ IAM role                       SSM Core + acesso mínimo ao bucket
         └─ IMDSv2 obrigatório

S3 bucket privado, AES256, lifecycle de 7 dias, force_destroy
SSM SecureString  /rollback-netcode/session-key
```

## Modelo de ameaça

A superfície de ataque inteira é **uma porta UDP, de um endereço IP**.

### Sem SSH

Não há par de chaves, não há regra para a porta 22, não há bastion. A
administração passa por SSM Session Manager, que não precisa de **nenhuma** regra
de entrada — o agente disca para fora.

```bash
aws ssm start-session --region eu-central-1 --target i-0abc123...
```

Um laboratório que abre a 22 "só para debugar" é um laboratório com um buraco
permanente.

### Sem dashboard exposto

O exportador Prometheus escuta em `127.0.0.1:9898`. Não há porta de métricas
aberta em nenhuma interface pública, em nenhuma das pontas. Os números do peer
remoto chegam pelo **próprio link da sessão**, como `TelemetrySummary`, e são
re-exportados localmente com `peer="remote"`.

É por isso que o `docker-compose` da observabilidade usa rede de host: um
contêiner na bridge padrão simplesmente não alcança um listener de loopback do
host, e a alternativa seria abrir o exportador em `0.0.0.0` — trocando a única
propriedade de segurança real do laboratório por uma conveniência de contêiner.

### `allowed_cidr` recusa `0.0.0.0/0`

```hcl
validation {
  condition     = var.allowed_cidr != "0.0.0.0/0"
  error_message = "Refusing to open the game port to the whole internet."
}
```

Não é sugestão. O Terraform falha.

Descubra seu endereço com `curl -s https://checkip.amazonaws.com` e use `/32`.

### IMDSv2 obrigatório

`http_tokens = "required"`, `http_put_response_hop_limit = 1`. A exigência do
token é o que impede uma leitura de deputado confuso das credenciais da instância
através de uma requisição que a aplicação foi enganada a fazer.

### IAM mínimo

A role tem `AmazonSSMManagedInstanceCore` e mais três permissões, cada uma
restrita ao recurso exato: listar **este** bucket, ler/escrever/apagar objetos
**deste** bucket, e ler **este** parâmetro.

### O systemd endurecido

O serviço do peer roda com `NoNewPrivileges`, `PrivateTmp`, `ProtectSystem=strict`
e `ProtectHome`, com escrita liberada apenas em `/opt/rollback/artifacts` e
`/opt/rollback/secrets`.

## A chave de sessão

Este é o ponto onde a maioria dos laboratórios vaza um segredo, então vale
detalhar o fluxo:

1. `just aws-up` gera 32 bytes de `/dev/urandom` **na sua máquina**.
2. Escreve em SSM como `SecureString` via `aws ssm put-parameter`.
3. Escreve numa cópia local `artifacts/session.key`, modo 0600.
4. A instância lê de SSM no `ExecStartPre` do serviço e grava em
   `/opt/rollback/secrets/session.key`, modo 0600, dono root.
5. O `run-peer.sh` exporta dali para o ambiente do processo.

O que **não** acontece:

- A chave nunca entra no estado do Terraform. O recurso `aws_ssm_parameter` é
  criado com um placeholder e tem `lifecycle { ignore_changes = [value] }`. Usar
  `random_password` colocaria o valor em claro no `.tfstate`.
- A chave nunca vira argumento de linha de comando. Argumentos são visíveis no
  `ps` para qualquer usuário da máquina. Ela vem de `ROLLBACK_SESSION_KEY` ou de
  `ROLLBACK_SESSION_KEY_FILE`.
- A chave nunca vai para `Environment=` de uma unit systemd, porque isso é
  legível por qualquer um via `systemctl show`.

E `just aws-down` apaga a cópia local: uma chave em disco sem sessão por trás é
só passivo.

## Desligamento automático

Duas camadas, porque instância esquecida é o modo de falha mais caro deste
laboratório:

```hcl
instance_initiated_shutdown_behavior = "terminate"
```

```bash
shutdown -h "+$((AUTO_SHUTDOWN_HOURS * 60))"
```

O user-data arma um `shutdown` para 4 horas depois do boot. Como o comportamento
de shutdown é *terminate* e não *stop*, o prazo é real — não é uma forma de
acumular instâncias paradas com volumes cobrando.

Ajustável via `auto_shutdown_hours` (1 a 12; acima disso o Terraform recusa).

## Instalação e execução remota

O user-data **não compila nada**. Uma t3.small compilando FBNeo levaria mais
tempo que a sessão inteira. Ele instala o mínimo, cria a estrutura em
`/opt/rollback`, arma o timer de sync de logs e para.

`just aws-up` então:

1. compila `rollback-bot` localmente (release);
2. sobe o binário — e, para SFA3, o core e a ROM — para o S3;
3. manda um comando via SSM que baixa tudo, escreve o `run-peer.sh` com os
   argumentos da sessão e dá `systemctl restart rollback-bot`.

## Sincronia de logs

Um timer do systemd roda `rollback-sync-logs` a cada minuto, e a unit também o
executa em `ExecStop` — ou seja, no caminho de desligamento, que é quando mais
importa. Assim `just collect` encontra um conjunto completo mesmo se a sessão
terminou mal.

## Por que Frankfurt

`eu-central-1` é a região do experimento, não um detalhe. Madrid–Frankfurt é
longe o suficiente para o rollback ser visível (RTT real de 30–45 ms, 2 a 3
frames) e perto o suficiente para o jogo continuar jogável. Mudar a região muda
o resultado.

## Por que t3.small, e quando trocar

t3.small (2 vCPU burstable, 2 GiB) sustenta a arena com folga enorme. Para SFA3 o
gargalo é o `retro_serialize` a cada frame, mais até 8 re-simulações num rollback
profundo.

Se o benchmark de fumaça não sustentar 60 Hz — olhe `effective_fps` no
`summary.csv`, ou `rollback_advance_seconds_total` no dashboard — troque para
`t3.medium`:

```hcl
instance_type = "t3.medium"
```

Sinais de que é hora: `effective_fps` bem abaixo de 60 no peer remoto, ou o tempo
somado de `advance + save_state` passando de ~8 ms por frame (metade do orçamento,
para deixar espaço para o pior caso de rollback).

## Fluxo completo

```bash
cp terraform/example.tfvars terraform/terraform.tfvars
$EDITOR terraform/terraform.tfvars    # allowed_cidr = $(curl -s https://checkip.amazonaws.com)/32

just aws-up arena                 # ~3 min: apply, chave, upload, start
just play arena                 # joga
just collect                          # SEMPRE antes de aws-down
just aws-down                         # destrói tudo
```

Para revisar uma mudança de infra sem aplicar:

```bash
just aws-plan
```

## Estado do Terraform

O backend é local (`terraform/terraform.tfstate`), e está no `.gitignore`.

Para um laboratório de uma pessoa isso é adequado. Se mais de uma pessoa for
operar a mesma conta, mova para um backend S3 com bloqueio em DynamoDB antes de
qualquer outra coisa — dois `apply` concorrentes sobre estado local produzem
recursos órfãos que só aparecem na fatura.
