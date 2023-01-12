import { EventHandler, MouseEventHandler, useState } from "react";
import { Form, Link, useLoaderData } from "react-router-dom";
import { Account } from "../../../bindings/Account";
import { Transaction } from "../../../bindings/Transaction";
import { UpdateAccount } from "../../../bindings/UpdateAccount";
import { getAccount, syncAccount, updateAccount } from "../api";
import Button from "../components/Button";
import Input from "../components/forms/Input";
import PageHeader from "../components/PageHeader";
import TransactionsList from "../components/TransactionsList";

type RouteParams = { id: number }

export async function action({ request, params }: { request: Request, params: RouteParams }) {
	const formData = await request.formData();
	return updateAccount(params.id, Object.fromEntries(formData.entries()) as unknown as UpdateAccount);
}
export async function loader({ params }: { params: RouteParams }) {
	return {
		account: await getAccount(params.id)
	}
}

export default function AccountSingle() {
	const { account } = useLoaderData() as { account: Account };
	const [isImportingTransaction, setIsImportingTransactions] = useState(false);
	const [importedTransactions, setImportedTransactions] = useState<Transaction[] | undefined>(undefined);

	async function onImportTransactions(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setIsImportingTransactions(true);
		let transactions = await syncAccount(account.id);
		setImportedTransactions(transactions);
		setIsImportingTransactions(false);
	}
	return <>
		<PageHeader><Link to="/accounts">Accounts</Link> &rarr; {account.name || '(untitled)'}</PageHeader>
		<main className="flex-grow p-10">
			<Form method="post" action={`/accounts/${account.id}`}>
				<label>
					<div className="text-xs text-gray-500 mb-2">Account Name</div>
					<Input name="name" placeholder="Enter name..." type="text" defaultValue={account.name} />
				</label>
				<div className="flex space-x-2 mt-4">
					<Button>Update Account</Button>
					<Button onClick={onImportTransactions} disabled={isImportingTransaction} isLoading={isImportingTransaction} varient="secondary">Manually Import Transactions</Button>
					<Button varient="danger">Delete Account</Button>
				</div>
			</Form>


			{importedTransactions &&
				<div className="mt-6">
					<p>Imported {importedTransactions.length} transactions!</p>
					{importedTransactions.length > 0 && <TransactionsList accounts={[]} transactions={importedTransactions} />}
				</div>
			}
		</main>
	</>
}
