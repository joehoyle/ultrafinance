import { EventHandler, MouseEventHandler, useState } from "react";
import { Form, Link, redirect, useLoaderData } from "react-router-dom";
import { Transaction } from "../../../bindings/Transaction";
import { deleteTransaction, getAccount, getTransaction } from "../api";
import Button from "../components/Button";
import PageHeader from "../components/PageHeader";

type RouteParams = { id: number }

export async function action({ request, params }: { request: Request, params: RouteParams }) {
	switch ( request.method ) {
		case 'DELETE': {
			await deleteTransaction(params.id);
			return redirect('/transactions');
		}
	}
}

export async function loader({ params }: { params: { id: number } }) {
	let transaction = await getTransaction(params.id)
	return {
		transaction,
		account: await getAccount( transaction.accountId ),
	}
}

export default function TransactionSingle() {
	const { transaction } = useLoaderData() as { transaction: Transaction };

	return <>
		<PageHeader><Link to="/transactions">Transactions</Link> &rarr; {transaction.id}</PageHeader>
		<main className="flex-grow p-10">
			<div className="text-sm mb-4">
				<h2>Details</h2>
				{ Object.keys( transaction ).map( key => (
					<div key={ key } className="flex border-b border-gray-100 py-2">
						<code className="flex-1">{ key }</code>
						<div className="flex-1">{ transaction[ key as keyof Transaction ] }</div>
					</div>
				)) }
			</div>
			<Form method="delete">
				<Button varient="danger">Delete Transaction</Button>
			</Form>
		</main>
	</>
}
