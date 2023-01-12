import { Transaction } from "../../../bindings/Transaction";
import { useNavigate } from "react-router-dom";
import { Account } from "../../../bindings/Account";

interface Props {
	transactions: Transaction[],
	accounts: Account[],
}
export default function TransactionsList({ transactions, accounts }: Props) {

	const sortedTransactions = transactions.sort( (a, b) => {
		const aDate = new Date( a.bookingDatetime || a.bookingDate );
		const bDate = new Date( b.bookingDatetime || b.bookingDate );
		return aDate > bDate ? -1 : 1;
	});

	const navigate = useNavigate();
	return <ul className="flex flex-col space-y-2">
		<table className="text-sm w-full">
			<thead className="text-xs text-gray-500">
				<tr>
					<td>Date</td>
					<td>Debtor / Creditor</td>
					<td>Amount</td>
					<td>Account</td>
				</tr>
			</thead>
			<tbody>
				{sortedTransactions.map(transaction => {
					const account = accounts.filter( a => a.id === transaction.accountId )[0];
					return <tr key={ transaction.id } onClick={() => navigate(`/transactions/${transaction.id}`)} className="border-b border-b-slate-50 hover:text-purple-800 hover:cursor-pointer">
						<td className="py-3">{transaction.bookingDate}</td>
						<td>{transaction.debtorName} {transaction.creditorName}</td>
						<td className={`text-xs ${Number(transaction.transactionAmount) < 0 && 'text-red-500'} ${Number(transaction.transactionAmount) > 0 && 'text-green-500'}`}>{transaction.transactionAmount} {transaction.transactionAmountCurrency}</td>
						<td>{ account && account.name }</td>
					</tr>
				})}
			</tbody>
		</table>
	</ul>
}
