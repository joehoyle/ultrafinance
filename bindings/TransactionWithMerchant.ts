// This file was generated by [ts-rs](https://github.com/Aleph-Alpha/ts-rs). Do not edit this file manually.
import type { Merchant } from "./Merchant";

export interface TransactionWithMerchant { id: number, externalId: string, creditorName: string | null, debtorName: string | null, remittanceInformation: string | null, bookingDate: string, bookingDatetime: string | null, transactionAmount: string, transactionAmountCurrency: string, proprietaryBankTransactionCode: string | null, currencyExchangeRate: string | null, currencyExchangeSourceCurrency: string | null, currencyExchangeTargetCurrency: string | null, merchantId: number | null, accountId: number, createdAt: string, updatedAt: string, merchant: Merchant | null, }