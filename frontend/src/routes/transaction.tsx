import { Await, Form, Link, Params, defer, redirect, useLoaderData, useParams } from "react-router-dom";
import { TransactionWithMerchant } from "../../../bindings/TransactionWithMerchant";
import { deleteTransaction, getTransaction } from "../api";
import Button from "../components/Button";
import PageHeader from "../components/PageHeader";
import { Suspense } from "react";

type RouteParams = Params;

export async function action({ request, params }: { request: Request, params: RouteParams }) {
	switch (request.method) {
		case 'DELETE': {
			await deleteTransaction(Number(params.id));
			return redirect('/transactions');
		}
	}
}

export async function loader({ params }: { params: RouteParams }) {
	return defer({
		transaction: getTransaction(Number(params.id)),
	})
}

export default function TransactionSingle() {
	const { transaction } = useLoaderData() as { transaction: TransactionWithMerchant };
	const params = useParams();

	return <>
		<PageHeader><Link to="/transactions">Transactions</Link> &rarr; {params.id}</PageHeader>
		<main className="flex-grow p-10">
			<Suspense fallback={<div />}>
				<Await resolve={transaction}>
					{(transaction: TransactionWithMerchant) => (
						<>
							{ transaction.merchant &&
								<div className="rounded bg-purple-50 p-4 mb-4 flex flex-row justify-between">
									<div>
										{ transaction.merchant.logo_url &&
											<img className="rounded w-16 h-16 " src={ transaction.merchant.logo_url } />
										}
										<h3 className="font-bold text-lg">{ transaction.merchant.name }</h3>
									</div>
									<div>
										{ [ transaction.merchant.location_structured?.city, transaction.merchant.location_structured?.country ].filter(Boolean).join(', ') }
										{ transaction.merchant.location_structured?.latitude && transaction.merchant.location_structured?.longitude &&
											<img width="150" className="rounded border border-purple-300" src={`https://api.mapbox.com/styles/v1/mapbox/light-v11/static/pin-s+555555(${ transaction.merchant.location_structured?.longitude },${ transaction.merchant.location_structured?.latitude })/${ transaction.merchant.location_structured?.longitude },${ transaction.merchant.location_structured?.latitude },11,0/300x200@2x?access_token=pk.eyJ1Ijoiam9laG95bGUiLCJhIjoiRzlMLUFQYyJ9.5EEMigt2JStBzNobiPo_9g`} />
										}
									</div>
								</div>
							}
							<div className="text-sm mb-4">
								<h2>Details</h2>
								{Object.keys(transaction).map(key => (
									<div key={key} className="flex border-b border-gray-100 py-2">
										<code className="flex-1">{key}</code>
										<div className="flex-1">{JSON.stringify(transaction[key as keyof TransactionWithMerchant])}</div>
									</div>
								))}
							</div>
							<Form method="delete">
								<Button varient="danger">Delete Transaction</Button>
							</Form>
						</>
					)}
				</Await>
			</Suspense>


		</main>
	</>
}
