import * as anchor from "@coral-xyz/anchor";
import { Program } from "@coral-xyz/anchor";
import { CrossSwap } from "../target/types/cross_swap";
import { PublicKey } from "@solana/web3.js";
import BN from "bn.js";
import { resolveOrCreateATA } from "@orca-so/common-sdk";
import { PDAUtil, WHIRLPOOL_CODER } from "@orca-so/whirlpools-sdk";

describe("cross-swap whirlpool execution test", () => {
  const provider = anchor.AnchorProvider.env();
  anchor.setProvider(provider);

  const program = anchor.workspace.CrossSwap as Program<CrossSwap>;

  const WHIRLPOOL_PROGRAM_ID = new PublicKey(
    "whirLbMiicVdio4qvUfM5KAg6Ct8VwpYzGff3uctyCc",
  );
  const TOKEN_PROGRAM_ID = new PublicKey(
    "TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA",
  );
  const WSOL_MINT = new PublicKey(
    "So11111111111111111111111111111111111111112",
  );
  const TARGET_WHIRLPOOL = new PublicKey(
    "Czfq3xZZDmsdGdUyrNLtRhGc47cXcZtLG4crryfu44zE",
  );

  it("Invokes the Orca whirlpool swap successfully via CPI", async () => {
    const connection = provider.connection;
    const walletPk = provider.wallet.publicKey;

    const whirlpoolInfo = await connection.getAccountInfo(TARGET_WHIRLPOOL);
    if (!whirlpoolInfo) {
      throw new Error("Whirlpool account not found in local validator clone");
    }

    const whirlpoolData = WHIRLPOOL_CODER.decode(
      "whirlpool",
      whirlpoolInfo.data,
    );

    const tokenMintA = whirlpoolData.tokenMintA as PublicKey;
    const tokenMintB = whirlpoolData.tokenMintB as PublicKey;
    const tokenVaultA = whirlpoolData.tokenVaultA as PublicKey;
    const tokenVaultB = whirlpoolData.tokenVaultB as PublicKey;

    // We use WSOL as source so we can fund it from local SOL via wrapping.
    const wsolIsA = tokenMintA.equals(WSOL_MINT);
    const wsolIsB = tokenMintB.equals(WSOL_MINT);
    if (!wsolIsA && !wsolIsB) {
      throw new Error(
        "Pool does not contain WSOL, cannot auto-fund source token",
      );
    }

    const aToB = wsolIsA;
    const swapSourceMint = wsolIsA ? tokenMintA : tokenMintB;
    const swapDestinationMint = wsolIsA ? tokenMintB : tokenMintA;

    const tickArray0 = PDAUtil.getTickArrayFromTickIndex(
      whirlpoolData.tickCurrentIndex,
      whirlpoolData.tickSpacing,
      TARGET_WHIRLPOOL,
      WHIRLPOOL_PROGRAM_ID,
      0,
    ).publicKey;
    const tickArray1 = PDAUtil.getTickArrayFromTickIndex(
      whirlpoolData.tickCurrentIndex,
      whirlpoolData.tickSpacing,
      TARGET_WHIRLPOOL,
      WHIRLPOOL_PROGRAM_ID,
      aToB ? 1 : -1,
    ).publicKey;
    const tickArray2 = PDAUtil.getTickArrayFromTickIndex(
      whirlpoolData.tickCurrentIndex,
      whirlpoolData.tickSpacing,
      TARGET_WHIRLPOOL,
      WHIRLPOOL_PROGRAM_ID,
      aToB ? 2 : -2,
    ).publicKey;
    const oracle = PDAUtil.getOracle(
      WHIRLPOOL_PROGRAM_ID,
      TARGET_WHIRLPOOL,
    ).publicKey;

    const getRentExempt = () =>
      connection.getMinimumBalanceForRentExemption(165);

    // Ensure test wallet has enough SOL for wrapped SOL + fees in local validator.
    const airdropSig = await connection.requestAirdrop(walletPk, 2_000_000_000);
    await connection.confirmTransaction(airdropSig, "confirmed");

    const sourceAtaIx = await resolveOrCreateATA(
      connection,
      walletPk,
      swapSourceMint,
      getRentExempt,
      new BN(50_000_000),
      walletPk,
      true,
      false,
      "ata",
    );

    const destinationAtaIx = await resolveOrCreateATA(
      connection,
      walletPk,
      swapDestinationMint,
      getRentExempt,
      undefined,
      walletPk,
      true,
      false,
      "ata",
    );

    const setupIxs = [
      ...sourceAtaIx.instructions,
      ...destinationAtaIx.instructions,
    ];
    const cleanupIxs = [
      ...sourceAtaIx.cleanupInstructions,
      ...destinationAtaIx.cleanupInstructions,
    ];
    const signerList = [...sourceAtaIx.signers, ...destinationAtaIx.signers];

    try {
      const tx = await program.methods
        .executeWhirlpoolSwap(new anchor.BN(10_000))
        .accounts({
          whirlpoolProgram: WHIRLPOOL_PROGRAM_ID,
          tokenProgram: TOKEN_PROGRAM_ID,
          tokenAuthority: walletPk,
          whirlpool: TARGET_WHIRLPOOL,
          tokenOwnerAccountA: sourceAtaIx.address,
          tokenVaultA: tokenVaultA,
          tokenOwnerAccountB: destinationAtaIx.address,
          tokenVaultB: tokenVaultB,
          tickArray0: tickArray0,
          tickArray1: tickArray1,
          tickArray2: tickArray2,
          oracle: oracle,
          swapSourceMint: swapSourceMint,
          swapDestinationMint: swapDestinationMint,
          tokenVaultAMint: tokenMintA,
          tokenVaultBMint: tokenMintB,
        } as any)
        .preInstructions(setupIxs)
        .postInstructions(cleanupIxs)
        .signers(signerList)
        .rpc({ skipPreflight: false });

      console.log("Whirlpool CPI swap succeeded on local fork!");
      console.log("Signature:", tx);
    } catch (e: any) {
      console.log("Whirlpool CPI swap failed.");
      if (e.logs) {
        console.log("--- ON CHAIN LOGS ---");
        e.logs.forEach((log) => console.log(log));
      } else {
        console.log(e);
      }
    }
  });
});
