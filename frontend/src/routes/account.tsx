import { EventHandler, MouseEventHandler, useState } from "react";
import { Form, Link, Params, redirect, useLoaderData } from "react-router-dom";
import { Account } from "../../../bindings/Account";
import { Transaction } from "../../../bindings/Transaction";
import { UpdateAccount } from "../../../bindings/UpdateAccount";
import { createRequisition, getAccount, syncAccount, updateAccount, deleteAccount } from "../api";
import Button from "../components/Button";
import Input from "../components/forms/Input";
import PageHeader from "../components/PageHeader";
import TransactionsList from "../components/TransactionsList";
import currencySymbols from "../utils/currency-symbols";

type RouteParams = Params;

export async function action({ request, params }: { request: Request, params: RouteParams }) {
	const formData = await request.formData();
	return updateAccount(Number(params.id), Object.fromEntries(formData.entries()) as unknown as UpdateAccount);
}
export async function loader({ params }: { params: RouteParams }) {
	return {
		account: await getAccount(Number(params.id))
	}
}

export default function AccountSingle() {
	const { account } = useLoaderData() as { account: Account };
	const [isImportingTransaction, setIsImportingTransactions] = useState(false);
	const [importingTransactionsError, setImportingTransactionsError] = useState<Error | undefined>(undefined);
	const [isRelinkingAccount, setIsRelinkingAccount] = useState(false);
	const [importedTransactions, setImportedTransactions] = useState<Transaction[] | undefined>(undefined);

	async function onImportTransactions(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setIsImportingTransactions(true);
		setImportingTransactionsError(undefined);
		try {

			let transactions = await syncAccount(account.id);
			setImportedTransactions(transactions);
		} catch (e) {
			setImportingTransactionsError(e as Error);
		}
		setIsImportingTransactions(false);
	}

	async function onRelinkAccount(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setIsRelinkingAccount(true);
		const requisition = await createRequisition(undefined, account.id);
		window.location.href = requisition.link;
	}

	async function onDeleteAccount(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		await deleteAccount(account.id);
		window.location.href = '/accounts';
	}

	return <>
		<PageHeader>
			<div className="flex flex-row justify-between">
				<div>
					<Link to="/accounts">Accounts</Link> &rarr; {account.name || '(untitled)'}
				</div>
				<div className="flex flex-row space-x-1">
					<span className="text-purple-500">{currencySymbols[account.currency as keyof typeof currencySymbols]}</span>
					<span>{account.balance}</span>
					<span className="text-gray-400">{account.currency}</span>
				</div>
			</div>
		</PageHeader>
		<main className="flex-grow p-10">
			<Form method="post" action={`/accounts/${account.id}`}>
				<label>
					<div className="text-xs text-gray-500 mb-2">Account Name</div>
					<Input name="name" placeholder="Enter name..." type="text" defaultValue={account.name} />
				</label>
				<div className="flex space-x-2 mt-4">
					<Button>Update Account</Button>
					<Button onClick={onRelinkAccount} disabled={isRelinkingAccount} isLoading={isRelinkingAccount} varient="secondary">Re-link Account</Button>
					<Button onClick={onImportTransactions} disabled={isImportingTransaction} isLoading={isImportingTransaction} varient="secondary">Manually Import Transactions</Button>
					<Button onClick={onDeleteAccount} varient="danger">Delete Account</Button>
				</div>

				{importingTransactionsError && <p className="text-red-500 p-2 bg-red-200 border-red-700 rounded mt-4">{importingTransactionsError.message}</p>}
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
