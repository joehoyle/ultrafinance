import { Await, defer, Link, useLoaderData, useRevalidator, useSearchParams } from "react-router-dom";
import PageHeader from "../components/PageHeader";

import { getAccounts, getTransactions, syncAccounts } from "../api";
import React, { Suspense, useState } from "react";
import LoadingList from "../components/LoadingList";
import TransactionsList from "../components/TransactionsList";
import { Account } from "../../../bindings/Account";
import Button from "../components/Button";
import { TransactionWithMerchant } from "../../../bindings/TransactionWithMerchant";

type Data = {
	transactions: TransactionWithMerchant[],
	accounts: Account[],
};

export async function action() {
	return syncAccounts();
}

export async function loader({request}: {request: Request}) {
	const url = new URL(request.url);
	return defer({
		transactions: getTransactions(url.searchParams.get('search') || undefined),
		accounts: getAccounts(),
	})
}

export default function Transactions() {
	const { transactions, accounts } = useLoaderData() as Data;
	const [ syncingTransactions, setSyncingTransactions ] = useState(false);
	const revalidator = useRevalidator();

	const [ searchParams, setSearchParams ] = useSearchParams();
	async function onSyncTransactions(e: React.MouseEvent<HTMLButtonElement, MouseEvent>) {
		e.preventDefault();
		setSyncingTransactions(true);
		await syncAccounts();
		setSyncingTransactions(false);
		revalidator.revalidate();
	}

	let debounceTimeout: number;

	function onKeyUpSearchInput(e: React.KeyboardEvent<HTMLInputElement>) {
		clearTimeout(debounceTimeout);
		let value = e.currentTarget.value;
		debounceTimeout = setTimeout(() => {
			console.log(value);
			setSearchParams({ search: value });
		}, 300);
	}
	console.log('rerendering');
	console.log(transactions)
	return <>
		<PageHeader>
			<div className="flex space-x-4">
				<div>Transactions</div>
				<input className="rounded flex-1 border border-gray-200 p-2 h-8 text-xs outline-none" placeholder="Search..." onKeyUp={ onKeyUpSearchInput } />
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


