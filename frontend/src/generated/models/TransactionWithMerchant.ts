/* generated using openapi-typescript-codegen -- do no edit */
/* istanbul ignore file */
/* tslint:disable */
/* eslint-disable */
export type TransactionWithMerchant = {
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
	merchant?: {
		created_at: string;
		external_id?: string;
		id: number;
		labels?: string;
		location?: string;
		location_structured?: {
			address?: string;
			apple_maps_url?: string;
			city?: string;
			country?: string;
			google_maps_url?: string;
			latitude?: number;
			longitude?: number;
			postcode?: string;
			state?: string;
			store_number?: number;
		};
		logo_url?: string;
		name: string;
		website?: string;
	};
	merchantId?: number;
	proprietaryBankTransactionCode?: string;
	remittanceInformation?: string;
	transactionAmount: string;
	transactionAmountCurrency: string;
	updatedAt: string;
	userId: number;
};
