//! An example of a multisig to execute arbitrary Solana transactions.
//!
//! This program can be used to allow a multisig to govern anything a regular
//! Pubkey can govern. One can use the multisig as a BPF program upgrade
//! authority, a mint authority, etc.
//!
//! To use, one must first create a `Multisig` account, specifying two important
//! parameters:
//!
//! 1. Owners - the set of addresses that sign transactions for the multisig.
//! 2. Threshold - the number of signers required to execute a transaction.
//!
//! Once the `Multisig` account is created, one can create a `Transaction`
//! account, specifying the parameters for a normal solana transaction.
//!
//! To sign, owners should invoke the `approve` instruction, and finally,
//! the `execute_transaction`, once enough (i.e. `threhsold`) of the owners have
//! signed.

use anchor_lang::prelude::*;
use anchor_lang::solana_program;
use anchor_lang::solana_program::instruction::Instruction;
use std::convert::Into;

#[program]
pub mod multisig {
    use super::*;

    // Initializes a new multisig account with a set of owners and a threshold.
    pub fn create_multisig(
        ctx: Context<CreateMultisig>,
        owners: Vec<Pubkey>,
        threshold: u64,
        nonce: u8,
    ) -> Result<()> {
        let multisig = &mut ctx.accounts.multisig;
        multisig.owners = owners;
        multisig.threshold = threshold;
        multisig.nonce = nonce;
        Ok(())
    }

    // Creates a new transaction account, automatically signed by the creator,
    // which must be one of the owners of the multisig.
    pub fn create_transaction(
        ctx: Context<CreateTransaction>,
        pid: Vec<Pubkey>,
        accs: Vec<Vec<TransactionAccount>>,
        data: Vec<Vec<u8>>,
    ) -> Result<()> {
        if pid.len() != accs.len() || pid.len() != data.len() {
            return Err(ErrorCode::ParamLength.into());
        }
        let owner_index = ctx
            .accounts
            .multisig
            .owners
            .iter()
            .position(|a| a == ctx.accounts.proposer.key)
            .ok_or(ErrorCode::InvalidOwner)?;

        let mut signers = Vec::new();
        signers.resize(ctx.accounts.multisig.owners.len(), false);
        signers[owner_index] = true;

        let tx = &mut ctx.accounts.transaction;
        tx.program_id = pid;
        tx.accounts = accs;
        tx.data = data;
        tx.signers = signers;
        tx.multisig = *ctx.accounts.multisig.to_account_info().key;
        tx.did_execute = false;

        Ok(())
    }

    // Approve and Executes the given transaction if threshold owners have signed it.
    pub fn approve(ctx: Context<Approve>) -> Result<()> {
        let owner_index = ctx
            .accounts
            .multisig
            .owners
            .iter()
            .position(|a| a == ctx.accounts.owner.key)
            .ok_or(ErrorCode::InvalidOwner)?;

        ctx.accounts.transaction.signers[owner_index] = true;

        // Do we have enough signers.
        let sig_count = ctx
            .accounts
            .transaction
            .signers
            .iter()
            .filter(|&did_sign| *did_sign)
            .count() as u64;
        if sig_count < ctx.accounts.multisig.threshold {
            return Ok(());
        }

        // Has this been executed already?
        if ctx.accounts.transaction.did_execute {
            return Err(ErrorCode::AlreadyExecuted.into());
        }

        // Execute the transaction signed by the multisig.
        let mut ixs: Vec<Instruction> = (&*ctx.accounts.transaction).into();
        for ix in ixs.iter_mut() {
            ix.accounts = ix
                .accounts
                .iter()
                .map(|acc| {
                    let mut acc = acc.clone();
                    if &acc.pubkey == ctx.accounts.multisig_signer.key {
                        acc.is_signer = true;
                    }
                    acc
                })
                .collect();
        }

        let seeds = &[
            ctx.accounts.multisig.to_account_info().key.as_ref(),
            &[ctx.accounts.multisig.nonce],
        ];
        let signer = &[&seeds[..]];
        let accounts = ctx.remaining_accounts;
        for ix in ixs.iter() {
            solana_program::program::invoke_signed(ix, &accounts, signer)?;
        }

        // Burn the transaction to ensure one time use.
        ctx.accounts.transaction.did_execute = true;

        Ok(())
    }
}

#[derive(Accounts)]
pub struct CreateMultisig<'info> {
    #[account(init)]
    multisig: ProgramAccount<'info, Multisig>,
    rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct CreateTransaction<'info> {
    multisig: ProgramAccount<'info, Multisig>,
    #[account(init)]
    transaction: ProgramAccount<'info, Transaction>,
    // One of the owners. Checked in the handler.
    #[account(signer)]
    proposer: AccountInfo<'info>,
    rent: Sysvar<'info, Rent>,
}

#[derive(Accounts)]
pub struct Approve<'info> {
    multisig: ProgramAccount<'info, Multisig>,
    #[account(seeds = [
        multisig.to_account_info().key.as_ref(),
        &[multisig.nonce],
    ])]
    multisig_signer: AccountInfo<'info>,
    #[account(mut, belongs_to = multisig)]
    transaction: ProgramAccount<'info, Transaction>,
    // One of the multisig owners. Checked in the handler.
    #[account(signer)]
    owner: AccountInfo<'info>,
}

#[account]
pub struct Multisig {
    owners: Vec<Pubkey>,
    threshold: u64,
    nonce: u8,
}

#[account]
pub struct Transaction {
    // The multisig account this transaction belongs to.
    multisig: Pubkey,
    // Target program to execute against.
    program_id: Vec<Pubkey>,
    // Accounts requried for the transaction.
    accounts: Vec<Vec<TransactionAccount>>,
    // Instruction data for the transaction.
    data: Vec<Vec<u8>>,
    // signers[index] is true iff multisig.owners[index] signed the transaction.
    signers: Vec<bool>,
    // Boolean ensuring one time execution.
    did_execute: bool,
}

impl From<&Transaction> for Vec<Instruction> {
    fn from(tx: &Transaction) -> Vec<Instruction> {
        let mut instructions: Vec<Instruction> = Vec::new();
        for (i, _pid) in tx.program_id.iter().enumerate() {
            instructions.push(Instruction {
                program_id: tx.program_id[i],
                accounts: tx.accounts[i].clone().into_iter().map(Into::into).collect(),
                data: tx.data[i].clone(),
            })
        }
        instructions
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Clone)]
pub struct TransactionAccount {
    pubkey: Pubkey,
    is_signer: bool,
    is_writable: bool,
}

impl From<TransactionAccount> for AccountMeta {
    fn from(account: TransactionAccount) -> AccountMeta {
        match account.is_writable {
            false => AccountMeta::new_readonly(account.pubkey, account.is_signer),
            true => AccountMeta::new(account.pubkey, account.is_signer),
        }
    }
}

#[error]
pub enum ErrorCode {
    #[msg("The given owner is not part of this multisig.")]
    InvalidOwner,
    #[msg("Not enough owners signed this transaction.")]
    NotEnoughSigners,
    #[msg("Cannot delete a transaction that has been signed by an owner.")]
    TransactionAlreadySigned,
    #[msg("Overflow when adding.")]
    Overflow,
    #[msg("Cannot delete a transaction the owner did not create.")]
    UnableToDelete,
    #[msg("The given transaction has already been executed.")]
    AlreadyExecuted,
    #[msg("Threshold must be less than or equal to the number of owners.")]
    InvalidThreshold,
    #[msg("program id account data must have same length")]
    ParamLength,
}
