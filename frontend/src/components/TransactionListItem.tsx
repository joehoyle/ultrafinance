import { Account } from "../../../bindings/Account";
import { TransactionWithMerchant } from "../../../bindings/TransactionWithMerchant";
import currencySymbols from "../utils/currency-symbols";
import { router } from '../main';

interface Props {
	transaction: TransactionWithMerchant,
	account?: Account,
}
export default function TransactionListItem({ transaction, account }: Props) {

	return (
		<div className="flex flex-row rounded hover:bg-purple-50 p-2 space-x-5 group" onClick={() => router.navigate(`/transactions/${transaction.id}`, {
			unstable_flushSync: true,
			unstable_viewTransition: false,
		})}>
			<div className="flex flex-row flex-1 space-x-5">
				{transaction.merchant?.logo_url ?
					<img className="border border-gray-200 rounded-full w-8 h-8 bg-gray-400 self-center group-hover:border-purple-500" src={transaction.merchant.logo_url} />
					:
					<div className="rounded-full w-8 h-8 bg-gray-100 self-center"></div>
				}
				<div className="flex flex-col max-w-28 sm:max-w-none text-sm sm:text-base">
					{transaction.merchant &&
						<div className="text-xs font-bold text-purple-800">{transaction.merchant.name}</div>
					}
					<div className="text-gray-500 break-words">{transaction.debtorName} {transaction.creditorName}</div>
				</div>
			</div>
			<div className="text-sm self-center">
				<div className="font-bold">
					<span className="text-gray-500">{currencySymbols[transaction.transactionAmountCurrency as keyof typeof currencySymbols]}</span>
					{Number(transaction.transactionAmount) < 0 ?
						<span className="text-red-700">{transaction.transactionAmount}</span>
						:
						<span className="text-purple-800">{transaction.transactionAmount}</span>
					}
					{' '}
					<span className="text-gray-300 text-xs">{transaction.transactionAmountCurrency}</span>
				</div>
				<div className="text-gray-400 text-xs text-right">{transaction.bookingDate}</div>
			</div>
			{account &&
				<div className="self-center w-20 text-xs">{account.name}</div>
			}
		</div>
	)
}
