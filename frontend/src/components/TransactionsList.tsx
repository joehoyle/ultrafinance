import { Transaction } from "../../../bindings/Transaction";
import { useNavigate } from "react-router-dom";
import { Account } from "../../../bindings/Account";
import { TransactionWithMerchant } from "../../../bindings/TransactionWithMerchant";
import TransactionListItem from "./TransactionListItem";

interface Props {
	transactions: TransactionWithMerchant[],
	accounts: Account[],
}
export default function TransactionsList({ transactions, accounts }: Props) {

	const sortedTransactions = transactions.sort( (a, b) => {
		const aDate = new Date( a.bookingDatetime || a.bookingDate );
		const bDate = new Date( b.bookingDatetime || b.bookingDate );
		return aDate > bDate ? -1 : 1;
	});

	return <div className="flex flex-col space-y-2">
		{ sortedTransactions.map( transaction => {
			const account = accounts.filter( a => a.id === transaction.accountId )[0];
			return <TransactionListItem key={ transaction.id } transaction={ transaction } account={ account } />
		} ) }
	</div>
}
