# Por que este build aplica um patch no FBNeo

## O sintoma

O core do FBNeo **não é determinístico entre processos**. Dois peers carregam a
mesma ROM, aplicam os mesmos inputs, e divergem antes do primeiro frame de
gameplay.

Medido no The Last Blade 2, com NVRAM limpa e o mesmo script de boot:

```
execução A    checksum f000300 e5467290974af991
execução B    checksum f000300 eaf4e5314c60aeed
```

E o teste que identifica a causa — duas execuções **iniciadas no mesmo segundo
de relógio**:

```
execução A    checksum f000300 59c82db8a4071bd1
execução B    checksum f000300 59c82db8a4071bd1
```

Idênticas. `time(NULL)` tem granularidade de um segundo, então isso é a
assinatura de uma dependência do relógio da máquina hospedeira.

## As duas dependências

Ambas em `src/burn/burn.cpp`:

```c
void BurnRandomInit()
{ // for states & input recordings - init before emulation starts
	if (is_netgame_or_recording()) {
		BurnRandomSetSeed(0x303808909313ULL);
	} else {
		BurnRandomSetSeed(time(NULL));      // <-- (1)
	}
}
```

```c
void BurnGetLocalTime(tm *nTime)
{
	if (is_netgame_or_recording()) {
		...
		nTime->tm_mday = 1; // 2018-06-01 00:00:00 for netgame
		...
	} else {
		time_t nLocalTime = time(NULL);     // <-- (2)
		...
	}
}
```

A (2) é a que morde o Neo Geo especificamente: o hardware tem um relógio de
calendário µPD4990A, e o driver o lê durante o boot do BIOS
(`src/burn/drv/neogeo/neo_upd4990a.cpp:54`). O valor entra no estado da máquina
antes de qualquer input existir.

## O patch

O FBNeo **já resolve as duas** — ele substitui semente fixa e data fixa quando
`is_netgame_or_recording()` é verdadeiro. Para o alvo libretro isso é
literalmente o global `kNetGame`:

```c
// src/burn/burnint.h
#else
inline static INT32 is_netgame_or_recording()
{
	return kNetGame;
}
#endif
```

E `src/burner/libretro/libretro.cpp` declara `int kNetGame = 0;` e nunca o
altera, porque um frontend comum não tem netplay. Este laboratório tem.

O patch inteiro é trocar esse zero por um:

```
-int kNetGame = 0;
+int kNetGame = 1;
```

O `Dockerfile` faz a troca com `sed` e **falha o build** se o padrão não
casar — um pin de commit que mude essa linha tem de quebrar ruidosamente, não
produzir silenciosamente um core não determinístico.

É o mesmo interruptor de que o Fightcade depende para rodar FBNeo com rollback,
ou seja, caminho conhecido e não gambiarra privada.

## O que isso custa

A máquina emulada acredita que são sempre **2018-06-01 00:00:00**.

Nada num jogo de luta depende da data. E para este laboratório a data fixa não é
um efeito colateral aceitável, é um **requisito**: um peer em Madri e um em
Frankfurt não compartilham relógio, e dessincronizariam num fuso horário
diferente mesmo que compartilhassem.

## Por que não dá para consertar de fora

`kNetGame` existe no binário como símbolo local (`b kNetGame` no `nm`), não
exportado dinamicamente. `dlsym` não o encontra. Dava para calcular o endereço
de execução a partir do offset da tabela de símbolos estáticos e escrever nele,
mas isso quebraria silenciosamente a cada rebuild do core — exatamente o tipo de
falha que este laboratório existe para tornar impossível.

Corrigir na fonte e recompilar é reprodutível, verificável pelo SHA-256 que o
handshake compara, e auditável no `fbneo-commit.txt`.
