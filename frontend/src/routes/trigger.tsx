import { Form, Link, redirect, useLoaderData } from "react-router-dom";
import { UpdateTrigger } from "../../../bindings/UpdateTrigger";
import { deleteTrigger, getFunctions, getTrigger, updateTrigger } from "../api";
import Button from "../components/Button";
import PageHeader from "../components/PageHeader";
import TriggerBuilder from "../components/TriggerBuilder";
import FormDataToJson from '../utils/FormDataToJson';

type RouteParams = { id: number }

export async function action({ request, params }: { request: Request, params: RouteParams }) {
	switch ( request.method ) {
		case 'POST':
			const formData = await request.formData();
			const data = FormDataToJson<UpdateTrigger>(formData);
			return updateTrigger(params.id, {
				...data,
				function_id: Number( data.function_id ),
			} as unknown as UpdateTrigger);
		case 'DELETE': {
			await deleteTrigger(params.id);
			return redirect('/triggers');
		}
	}
}

export async function loader({ params }: { params: RouteParams }) {
	return {
		trigger: await getTrigger(params.id),
		functions: await getFunctions(),
	}
}

export default function TriggerSingle() {
	const { trigger, functions } = useLoaderData() as Awaited<ReturnType<typeof loader>>;

	return <>
		<PageHeader><Link to="/triggers">Triggers</Link> &rarr; {trigger.name}</PageHeader>
		<main className="flex-grow p-10">
			<Form method="post">
				<TriggerBuilder functions={ functions } trigger={ trigger } />
				<div className="flex space-x-2 mt-4">
					<Button>Update Trigger</Button>
				</div>
			</Form>

			<Form className="mt-4" method="delete" action={`/triggers/${ trigger.id }`}>
				<Button varient="danger">Delete Trigger</Button>
			</Form>
		</main>
	</>
}


