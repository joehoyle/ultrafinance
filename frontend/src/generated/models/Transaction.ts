/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type Transaction = {
	accountId: number;
	bookingDate: string;
	bookingDatetime?: string;
	createdAt: string;
	creditorName?: string;
	currencyExchangeRate?: string;
	currencyExchangeSourceCurrency?: string;
	currencyExchangeTargetCurrency?: string;
	debtorName?: string;
	externalId: string;
	id: number;
	merchantId?: number;
	proprietaryBankTransactionCode?: string;
	remittanceInformation?: string;
	transactionAmount: string;
	transactionAmountCurrency: string;
	updatedAt: string;
	userId: number;
};
