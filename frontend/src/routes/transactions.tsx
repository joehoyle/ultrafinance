import { Await, defer, Link, useLoaderData, useRevalidator } from "react-router-dom";
import PageHeader from "../components/PageHeader";
import { useNavigate } from "react-router-dom";

import type { Transaction } from "../../../bindings/Transaction";
import { getAccounts, getTransactions, syncAccounts } from "../api";
import React, { Suspense, useState } from "react";
import LoadingList from "../components/LoadingList";
import TransactionsList from "../components/TransactionsList";
import { Account } from "../../../bindings/Account";
import Button from "../components/Button";

type Data = {
	transactions: Transaction[],
	accounts: Account[],
};

export async function action() {
	return syncAccounts();
}

export async function loader() {
	return defer({
		transactions: getTransactions(),
		accounts: getAccounts(),
	})
}

export default function Transactions() {
	const { transactions, accounts } = useLoaderData() as Data;
	const [ syncingTransactions, setSyncingTransactions ] = useState(false);
	const revalidator = useRevalidator();

	async function onSyncTransactions(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setSyncingTransactions(true);
		await syncAccounts();
		setSyncingTransactions(false);
		revalidator.revalidate();
	}
	return <>
		<PageHeader>
			<div className="flex">
				<div className="flex-1">Transactions</div>
				<Button onClick={ onSyncTransactions } disabled={ syncingTransactions } varient="secondary" isLoading={ syncingTransactions }>Import Transactions</Button>
			</div>
		</PageHeader>
		<main className=" flex-grow p-10">
			<Suspense fallback={<LoadingList number={15} />}>
				<Await resolve={Promise.all([transactions, accounts])}>
					{([transactions, accounts]) => <TransactionsList accounts={accounts} transactions={transactions} />}
				</Await>
			</Suspense>

		</main>
	</>
}


